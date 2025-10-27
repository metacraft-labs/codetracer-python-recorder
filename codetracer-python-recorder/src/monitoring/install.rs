//! Tracer installation plumbing backed by the callbacks module.

use crate::code_object::CodeObjectRegistry;
use crate::ffi;
use log::warn;
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
    let mut guard = GLOBAL.lock().unwrap();
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
    let mut guard = GLOBAL.lock().unwrap();
    uninstall_locked(py, &mut guard)
}

/// Flush the currently installed tracer if any.
pub fn flush_installed_tracer(py: Python<'_>) -> PyResult<()> {
    if let Some(global) = GLOBAL.lock().unwrap().as_mut() {
        global.tracer.flush(py)?;
    }
    Ok(())
}

/// Provide the session exit status to the active tracer if one is installed.
pub fn update_exit_status(py: Python<'_>, exit_code: Option<i32>) -> PyResult<()> {
    if let Some(global) = GLOBAL.lock().unwrap().as_mut() {
        global.tracer.set_exit_status(py, exit_code)?;
    }
    Ok(())
}
