//! Event handling pipeline for `RuntimeTracer`.

use super::runtime_tracer::RuntimeTracer;
use crate::code_object::CodeObjectWrapper;
use crate::ffi;
use crate::logging::with_error_code;
use crate::monitoring::{
    events_union, CallbackOutcome, CallbackResult, EventSet, MonitoringEvents, Tracer,
};
use crate::policy::policy_snapshot;
use crate::runtime::activation::ActivationExitKind;
use crate::runtime::frame_inspector::capture_frame;
use crate::runtime::io_capture::ScopedMuteIoCapture;
use crate::runtime::line_snapshots::FrameId;
use crate::runtime::logging::log_event;
use crate::runtime::value_capture::{
    capture_call_arguments, encode_named_argument, record_return_value, record_visible_scope,
};
use pyo3::prelude::*;
use pyo3::types::PyAny;
use recorder_errors::{bug, enverr, target, ErrorCode};
use runtime_tracing::{FullValueRecord, Line, PathId, TraceEventsFileFormat, TraceWriter};
use std::collections::HashSet;
use std::path::Path;
use std::thread;

#[cfg(feature = "integration-test")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "integration-test")]
use std::sync::OnceLock;

#[cfg(feature = "integration-test")]
static FAILURE_MODE: OnceLock<Option<FailureMode>> = OnceLock::new();
#[cfg(feature = "integration-test")]
static FAILURE_TRIGGERED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FailureStage {
    PyStart,
    Line,
    Finish,
}

impl FailureStage {
    fn as_str(self) -> &'static str {
        match self {
            FailureStage::PyStart => "py_start",
            FailureStage::Line => "line",
            FailureStage::Finish => "finish",
        }
    }
}

// Failure injection helpers are only compiled for integration tests.
#[cfg_attr(not(feature = "integration-test"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FailureMode {
    Stage(FailureStage),
    SuppressEvents,
    TargetArgs,
    Panic,
}

#[cfg(feature = "integration-test")]
fn configured_failure_mode() -> Option<FailureMode> {
    *FAILURE_MODE.get_or_init(|| {
        let raw = std::env::var("CODETRACER_TEST_INJECT_FAILURE").ok();
        if let Some(value) = raw.as_deref() {
            let _mute = ScopedMuteIoCapture::new();
            log::debug!("[RuntimeTracer] test failure injection mode: {}", value);
        }
        raw.and_then(|raw| match raw.trim().to_ascii_lowercase().as_str() {
            "py_start" | "py-start" => Some(FailureMode::Stage(FailureStage::PyStart)),
            "line" => Some(FailureMode::Stage(FailureStage::Line)),
            "finish" => Some(FailureMode::Stage(FailureStage::Finish)),
            "suppress-events" | "suppress_events" | "suppress" => Some(FailureMode::SuppressEvents),
            "target" | "target-args" | "target_args" => Some(FailureMode::TargetArgs),
            "panic" | "panic-callback" | "panic_callback" => Some(FailureMode::Panic),
            _ => None,
        })
    })
}

#[cfg(feature = "integration-test")]
fn should_inject_failure(stage: FailureStage) -> bool {
    matches!(configured_failure_mode(), Some(FailureMode::Stage(mode)) if mode == stage)
        && mark_failure_triggered()
}

#[cfg(not(feature = "integration-test"))]
fn should_inject_failure(_stage: FailureStage) -> bool {
    false
}

#[cfg(feature = "integration-test")]
fn should_inject_target_error() -> bool {
    matches!(configured_failure_mode(), Some(FailureMode::TargetArgs)) && mark_failure_triggered()
}

#[cfg(not(feature = "integration-test"))]
fn should_inject_target_error() -> bool {
    false
}

#[cfg(feature = "integration-test")]
fn should_panic_in_callback() -> bool {
    matches!(configured_failure_mode(), Some(FailureMode::Panic)) && mark_failure_triggered()
}

#[cfg(not(feature = "integration-test"))]
#[allow(dead_code)]
fn should_panic_in_callback() -> bool {
    false
}

#[cfg(feature = "integration-test")]
fn mark_failure_triggered() -> bool {
    !FAILURE_TRIGGERED.swap(true, Ordering::SeqCst)
}

#[cfg(not(feature = "integration-test"))]
#[allow(dead_code)]
fn mark_failure_triggered() -> bool {
    false
}

#[cfg(feature = "integration-test")]
fn injected_failure_err(stage: FailureStage) -> PyErr {
    let err = bug!(
        ErrorCode::TraceIncomplete,
        "test-injected failure at {}",
        stage.as_str()
    )
    .with_context("injection_stage", stage.as_str().to_string());
    ffi::map_recorder_error(err)
}

#[cfg(not(feature = "integration-test"))]
fn injected_failure_err(stage: FailureStage) -> PyErr {
    let err = bug!(
        ErrorCode::TraceIncomplete,
        "failure injection requested at {} without fail-injection feature",
        stage.as_str()
    )
    .with_context("injection_stage", stage.as_str().to_string());
    ffi::map_recorder_error(err)
}

#[cfg(feature = "integration-test")]
pub(super) fn suppress_events() -> bool {
    matches!(configured_failure_mode(), Some(FailureMode::SuppressEvents))
}

#[cfg(not(feature = "integration-test"))]
pub(super) fn suppress_events() -> bool {
    false
}

impl Tracer for RuntimeTracer {
    fn interest(&self, events: &MonitoringEvents) -> EventSet {
        // Balanced call stack requires tracking yields, resumes, throws, and unwinds
        events_union(&[
            events.PY_START,
            events.PY_RETURN,
            events.PY_YIELD,
            events.PY_UNWIND,
            events.PY_RESUME,
            events.PY_THROW,
            events.LINE,
        ])
    }

    fn on_py_start(
        &mut self,
        py: Python<'_>,
        code: &CodeObjectWrapper,
        _offset: i32,
    ) -> CallbackResult {
        if let Some(outcome) = self.evaluate_gate(py, code, true) {
            return Ok(outcome);
        }

        if should_inject_failure(FailureStage::PyStart) {
            return Err(injected_failure_err(FailureStage::PyStart));
        }

        if should_inject_target_error() {
            return Err(ffi::map_recorder_error(
                target!(
                    ErrorCode::TraceIncomplete,
                    "test-injected target error from capture_call_arguments"
                )
                .with_context("injection_stage", "capture_call_arguments"),
            ));
        }

        log_event(py, code, "on_py_start", None);

        let scope_resolution = self.filter.cached_resolution(code.id());
        let value_policy = scope_resolution.as_ref().map(|res| res.value_policy());
        let wants_telemetry = value_policy.is_some();

        let mut telemetry_holder = if wants_telemetry {
            Some(self.filter.values_mut())
        } else {
            None
        };
        let telemetry = telemetry_holder.as_deref_mut();
        match capture_call_arguments(py, &mut self.writer, code, value_policy, telemetry) {
            Ok(args) => self.register_call_record(py, code, args),
            Err(err) => {
                let details = err.to_string();
                with_error_code(ErrorCode::FrameIntrospectionFailed, || {
                    let _mute = ScopedMuteIoCapture::new();
                    log::error!("on_py_start: failed to capture args: {details}");
                });
                return Err(ffi::map_recorder_error(
                    enverr!(
                        ErrorCode::FrameIntrospectionFailed,
                        "failed to capture call arguments"
                    )
                    .with_context("details", details),
                ));
            }
        }

        Ok(CallbackOutcome::Continue)
    }

    fn on_py_resume(
        &mut self,
        py: Python<'_>,
        code: &CodeObjectWrapper,
        _offset: i32,
    ) -> CallbackResult {
        if let Some(outcome) = self.evaluate_gate(py, code, false) {
            return Ok(outcome);
        }

        log_event(py, code, "on_py_resume", None);
        self.register_call_record(py, code, Vec::new());
        Ok(CallbackOutcome::Continue)
    }

    fn on_line(&mut self, py: Python<'_>, code: &CodeObjectWrapper, lineno: u32) -> CallbackResult {
        if let Some(outcome) = self.evaluate_gate(py, code, false) {
            return Ok(outcome);
        }

        if should_inject_failure(FailureStage::Line) {
            return Err(injected_failure_err(FailureStage::Line));
        }

        #[cfg(feature = "integration-test")]
        {
            if should_panic_in_callback() {
                panic!("test-injected panic in on_line");
            }
        }

        log_event(py, code, "on_line", Some(lineno));

        self.flush_io_before_step(thread::current().id());

        let scope_resolution = self.filter.cached_resolution(code.id());
        let value_policy = scope_resolution.as_ref().map(|res| res.value_policy());
        let wants_telemetry = value_policy.is_some();

        let line_value = Line(lineno as i64);
        let mut recorded_path: Option<(PathId, Line)> = None;

        if let Ok(filename) = code.filename(py) {
            let path = Path::new(filename);
            let path_id = TraceWriter::ensure_path_id(&mut self.writer, path);
            TraceWriter::register_step(&mut self.writer, path, line_value);
            self.mark_event();
            recorded_path = Some((path_id, line_value));
        }

        let snapshot = capture_frame(py, code)?;

        if let Some((path_id, line)) = recorded_path {
            let frame_id = FrameId::from_raw(snapshot.frame_ptr() as usize as u64);
            self.io
                .record_snapshot(thread::current().id(), path_id, line, frame_id);
        }

        let mut recorded: HashSet<String> = HashSet::new();
        let mut telemetry_holder = if wants_telemetry {
            Some(self.filter.values_mut())
        } else {
            None
        };
        let telemetry = telemetry_holder.as_deref_mut();
        record_visible_scope(
            py,
            &mut self.writer,
            &snapshot,
            &mut recorded,
            value_policy,
            telemetry,
        );

        Ok(CallbackOutcome::Continue)
    }

    fn on_py_return(
        &mut self,
        py: Python<'_>,
        code: &CodeObjectWrapper,
        _offset: i32,
        retval: &Bound<'_, PyAny>,
    ) -> CallbackResult {
        self.handle_return_edge(
            py,
            code,
            "on_py_return",
            retval,
            None,
            Some(ActivationExitKind::Completed),
            true,
        )
    }

    fn on_py_yield(
        &mut self,
        py: Python<'_>,
        code: &CodeObjectWrapper,
        _offset: i32,
        retval: &Bound<'_, PyAny>,
    ) -> CallbackResult {
        self.handle_return_edge(
            py,
            code,
            "on_py_yield",
            retval,
            Some("<yield>"),
            Some(ActivationExitKind::Suspended),
            false,
        )
    }

    fn on_py_throw(
        &mut self,
        py: Python<'_>,
        code: &CodeObjectWrapper,
        _offset: i32,
        exception: &Bound<'_, PyAny>,
    ) -> CallbackResult {
        if let Some(outcome) = self.evaluate_gate(py, code, false) {
            return Ok(outcome);
        }

        log_event(py, code, "on_py_throw", None);

        let scope_resolution = self.filter.cached_resolution(code.id());
        let value_policy = scope_resolution.as_ref().map(|res| res.value_policy());
        let wants_telemetry = value_policy.is_some();

        let mut telemetry_holder = if wants_telemetry {
            Some(self.filter.values_mut())
        } else {
            None
        };
        let telemetry = telemetry_holder.as_deref_mut();

        let mut args: Vec<FullValueRecord> = Vec::new();
        if let Some(arg) = encode_named_argument(
            py,
            &mut self.writer,
            exception,
            "exception",
            value_policy,
            telemetry,
        ) {
            args.push(arg);
        }
        self.register_call_record(py, code, args);

        Ok(CallbackOutcome::Continue)
    }

    fn on_py_unwind(
        &mut self,
        py: Python<'_>,
        code: &CodeObjectWrapper,
        _offset: i32,
        exception: &Bound<'_, PyAny>,
    ) -> CallbackResult {
        self.handle_return_edge(
            py,
            code,
            "on_py_unwind",
            exception,
            Some("<unwind>"),
            Some(ActivationExitKind::Completed),
            false,
        )
    }

    fn set_exit_status(
        &mut self,
        _py: Python<'_>,
        exit_code: Option<i32>,
    ) -> PyResult<()> {
        self.record_exit_status(exit_code);
        Ok(())
    }

    fn notify_failure(&mut self, _py: Python<'_>) -> PyResult<()> {
        self.mark_disabled();
        Ok(())
    }

    fn flush(&mut self, _py: Python<'_>) -> PyResult<()> {
        // Trace event entry
        let _mute = ScopedMuteIoCapture::new();
        log::debug!("[RuntimeTracer] flush");
        drop(_mute);
        self.flush_pending_io();
        // For non-streaming formats we can update the events file.
        match self.format {
            TraceEventsFileFormat::Json | TraceEventsFileFormat::BinaryV0 => {
                TraceWriter::finish_writing_trace_events(&mut self.writer).map_err(|err| {
                    ffi::map_recorder_error(
                        enverr!(ErrorCode::Io, "failed to finalise trace events")
                            .with_context("source", err.to_string()),
                    )
                })?;
            }
            TraceEventsFileFormat::Binary => {
                // Streaming writer: no partial flush to avoid closing the stream.
            }
        }
        self.filter.clear_caches();
        Ok(())
    }

    fn finish(&mut self, py: Python<'_>) -> PyResult<()> {
        // Trace event entry
        let _mute_finish = ScopedMuteIoCapture::new();
        log::debug!("[RuntimeTracer] finish");

        if should_inject_failure(FailureStage::Finish) {
            return Err(injected_failure_err(FailureStage::Finish));
        }

        let _trace_scope = self.lifecycle.trace_id_scope();
        let policy = policy_snapshot();

        if self.io.teardown(py, &mut self.writer) {
            self.mark_event();
        }

        self.emit_session_exit(py);

        let exit_summary = self.exit_summary();

        if self.lifecycle.encountered_failure() {
            if policy.keep_partial_trace {
                if let Err(err) = self.lifecycle.finalise(&mut self.writer, &self.filter, &exit_summary)
                {
                    with_error_code(ErrorCode::TraceIncomplete, || {
                        log::warn!(
                            "failed to finalise partial trace after disable: {}",
                            err.message()
                        );
                    });
                }
                if let Some(outputs) = self.lifecycle.output_paths() {
                    with_error_code(ErrorCode::TraceIncomplete, || {
                        log::warn!(
                            "recorder detached after failure; keeping partial trace at {}",
                            outputs.events().display()
                        );
                    });
                }
            } else {
                self.lifecycle
                    .cleanup_partial_outputs()
                    .map_err(ffi::map_recorder_error)?;
            }
            self.function_ids.clear();
            self.module_names.clear();
            self.io.clear_snapshots();
            self.filter.reset();
            self.lifecycle.reset_event_state();
            return Ok(());
        }

        self.lifecycle
            .require_trace_or_fail(&policy)
            .map_err(ffi::map_recorder_error)?;
        self.lifecycle
            .finalise(&mut self.writer, &self.filter, &exit_summary)
            .map_err(ffi::map_recorder_error)?;
        self.function_ids.clear();
        self.module_names.clear();
        self.filter.reset();
        self.io.clear_snapshots();
        self.lifecycle.reset_event_state();
        Ok(())
    }
}

impl RuntimeTracer {
    fn register_call_record(
        &mut self,
        py: Python<'_>,
        code: &CodeObjectWrapper,
        args: Vec<FullValueRecord>,
    ) {
        if let Ok(fid) = self.ensure_function_id(py, code) {
            TraceWriter::register_call(&mut self.writer, fid, args);
            self.mark_event();
        }
    }

    fn handle_return_edge(
        &mut self,
        py: Python<'_>,
        code: &CodeObjectWrapper,
        label: &'static str,
        retval: &Bound<'_, PyAny>,
        capture_label: Option<&'static str>,
        exit_kind: Option<ActivationExitKind>,
        allow_disable: bool,
    ) -> CallbackResult {
        if let Some(outcome) = self.evaluate_gate(py, code, allow_disable) {
            return Ok(outcome);
        }

        log_event(py, code, label, None);

        self.flush_pending_io();

        let scope_resolution = self.filter.cached_resolution(code.id());
        let value_policy = scope_resolution.as_ref().map(|res| res.value_policy());
        let wants_telemetry = value_policy.is_some();
        let object_name = scope_resolution.as_ref().and_then(|res| res.object_name());

        let mut telemetry_holder = if wants_telemetry {
            Some(self.filter.values_mut())
        } else {
            None
        };
        let telemetry = telemetry_holder.as_deref_mut();

        let candidate_name = capture_label.map(|label| label as &str).or(object_name);

        record_return_value(
            py,
            &mut self.writer,
            retval,
            value_policy,
            telemetry,
            candidate_name,
        );
        self.mark_event();

        if let Some(kind) = exit_kind {
            if self.lifecycle.activation_mut().handle_exit(code.id(), kind) {
                let _mute = ScopedMuteIoCapture::new();
                log::debug!("[RuntimeTracer] deactivated on activation return");
            }
        }

        Ok(CallbackOutcome::Continue)
    }
}
