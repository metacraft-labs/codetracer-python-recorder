//! Tracer installation plumbing backed by the callbacks module.

use crate::code_object::CodeObjectRegistry;
use crate::ffi;
use codetracer_trace_writer_nim::SpanRecord;
use log::warn;
use std::time::{Duration, Instant};
use pyo3::{prelude::*, types::PyModule};
use recorder_errors::{usage, ErrorCode};

use super::api::Tracer;
use super::callbacks::{self, Global, GLOBAL};
use super::{acquire_tool_id, free_tool_id, monitoring_events, set_events, NO_EVENTS};

pub(super) fn uninstall_locked(py: Python<'_>, guard: &mut Option<Global>) -> PyResult<()> {
    if let Some(mut global) = guard.take() {
        let finish_result = global.tracer.finish(py);

        let cleanup_result = (|| -> PyResult<()> {
            let events = monitoring_events(py)?;
            callbacks::unregister_enabled_callbacks(py, &global.tool, &global.mask, events)?;
            set_events(py, &global.tool, NO_EVENTS)?;
            free_tool_id(py, &global.tool)?;
            Ok(())
        })();

        if let Err(err) = finish_result {
            if let Err(cleanup_err) = cleanup_result {
                warn!(
                    "failed to reset monitoring callbacks after finish error: {}",
                    cleanup_err
                );
            }
            return Err(err);
        }

        cleanup_result?;
    }
    Ok(())
}

/// Install a tracer and hook it into Python's `sys.monitoring`.
pub fn install_tracer(py: Python<'_>, tracer: Box<dyn Tracer>) -> PyResult<()> {
    let mut guard = GLOBAL.lock().expect("GLOBAL mutex poisoned");
    if guard.is_some() {
        return Err(ffi::map_recorder_error(usage!(
            ErrorCode::TracerInstallConflict,
            "tracer already installed"
        )));
    }

    let tool = acquire_tool_id(py)?;
    let events = monitoring_events(py)?;
    let monitoring = py.import("sys")?.getattr("monitoring")?;
    let disable_sentinel = monitoring.getattr("DISABLE")?.unbind();

    let module = PyModule::new(py, "_codetracer_callbacks")?;

    let mask = tracer.interest(events);
    callbacks::register_enabled_callbacks(py, &module, &tool, &mask, events)?;

    set_events(py, &tool, mask)?;

    *guard = Some(Global {
        registry: CodeObjectRegistry::default(),
        tracer,
        mask,
        tool,
        disable_sentinel,
    });
    Ok(())
}

/// Remove the installed tracer if any.
pub fn uninstall_tracer(py: Python<'_>) -> PyResult<()> {
    let mut guard = GLOBAL.lock().expect("GLOBAL mutex poisoned");
    uninstall_locked(py, &mut guard)
}

/// Flush the currently installed tracer if any.
pub fn flush_installed_tracer(py: Python<'_>) -> PyResult<()> {
    if let Some(global) = GLOBAL.lock().expect("GLOBAL mutex poisoned").as_mut() {
        global.tracer.flush(py)?;
    }
    Ok(())
}

/// How long a span entry point waits for [`GLOBAL`] before it starts complaining.
///
/// Not a timeout: the lock is only ever held for the duration of one monitoring
/// callback, so exceeding this means something else is wrong and the operator
/// should hear about it.
const SPAN_LOCK_WARN_AFTER: Duration = Duration::from_secs(1);

/// Acquire [`GLOBAL`] from a thread that is NOT inside a monitoring callback,
/// without deadlocking against one that is.
///
/// **Why this is not `GLOBAL.lock()`.**  Two lock orders exist in this process
/// and each one deadlocks on its own:
///
/// * *GIL then `GLOBAL`* — a monitoring callback holds the GIL and takes
///   `GLOBAL`, then runs Python-level work (frame introspection, `repr`) during
///   which CPython's eval loop happily hands the GIL to another thread.  If that
///   other thread now blocks on `GLOBAL` **while holding the GIL**, the callback
///   can never resume: it needs the GIL back to finish and release `GLOBAL`.
/// * *`GLOBAL` then GIL* — releasing the GIL around a blocking `GLOBAL.lock()`
///   (the obvious "fix") produces the mirror image: this thread ends up holding
///   `GLOBAL` and waiting for the GIL, while the callback thread holds the GIL
///   and waits for `GLOBAL`.
///
/// Measured, not theorised: with either of those, one HTTP request served on a
/// worker thread of a thread-per-request WSGI server hung the whole process —
/// caught by `test_threaded_wsgi_requests_land_in_span_stream`.
///
/// So the wait is a `try_lock` spin that holds NEITHER lock while it waits: the
/// GIL is released between attempts so the callback thread can make progress,
/// and `GLOBAL` is only ever held while the GIL is also held (the same order the
/// callbacks use), for the few microseconds the writer call takes.
fn lock_global_without_deadlock<T>(
    py: Python<'_>,
    operation: impl FnOnce(&mut Option<Global>) -> T,
) -> Result<T, String> {
    let start = Instant::now();
    let mut warned = false;
    let mut backoff = Duration::from_micros(20);
    loop {
        match GLOBAL.try_lock() {
            Ok(mut guard) => return Ok(operation(&mut guard)),
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err("GLOBAL mutex poisoned".to_string())
            }
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
        if !warned && start.elapsed() > SPAN_LOCK_WARN_AFTER {
            warned = true;
            warn!(
                "span registration has waited {:?} for the tracer lock; a monitoring \
                 callback appears to be stuck",
                start.elapsed()
            );
        }
        // The GIL MUST be released here — that is the whole point of the spin.
        py.allow_threads(|| std::thread::sleep(backoff));
        backoff = (backoff * 2).min(Duration::from_millis(1));
    }
}

/// RS-M5: append a span to the installed tracer's trace container.
///
/// Returns `Ok(false)` when no tracer is installed — a middleware wrapping an
/// app that is simply not being recorded is not an error, it just has nowhere
/// to put the span.  A tracer that IS installed and rejects the span yields
/// `Err`, so a recorded session cannot lose requests silently.
///
/// Safe to call from any thread; see [`lock_global_without_deadlock`] for the
/// concurrency contract, which is not obvious and was not free.
pub fn register_span_on_installed_tracer(py: Python<'_>, span: &SpanRecord) -> Result<bool, String> {
    lock_global_without_deadlock(py, |guard| match guard.as_mut() {
        Some(global) => global.tracer.register_span(span).map(|()| true),
        None => Ok(false),
    })?
}

/// RS-M5: the step index the next recorded event will occupy, or `None` when no
/// tracer is installed.  Same concurrency contract as
/// [`register_span_on_installed_tracer`].
pub fn installed_tracer_next_step_index(py: Python<'_>) -> Option<u64> {
    lock_global_without_deadlock(py, |guard| {
        guard.as_ref().and_then(|global| global.tracer.next_step_index())
    })
    .unwrap_or(None)
}

/// Provide the session exit status to the active tracer if one is installed.
pub fn update_exit_status(py: Python<'_>, exit_code: Option<i32>) -> PyResult<()> {
    if let Some(global) = GLOBAL.lock().expect("GLOBAL mutex poisoned").as_mut() {
        global.tracer.set_exit_status(py, exit_code)?;
    }
    Ok(())
}
