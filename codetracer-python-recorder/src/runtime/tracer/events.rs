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
use crate::runtime::assignment_reconstructor::{LineAssignment, RValueShape};
use crate::runtime::frame_inspector::capture_frame;
use crate::runtime::io_capture::ScopedMuteIoCapture;
use crate::runtime::line_snapshots::FrameId;
use crate::runtime::logging::log_event;
use crate::runtime::value_capture::{
    capture_call_arguments, encode_named_argument, record_return_value_streaming,
    record_visible_scope_streaming,
};
use crate::trace_filter::config::ValueAction;
use crate::trace_filter::engine::{ValueKind, ValuePolicy};
use codetracer_trace_types::{
    AssignmentRecord, BindVariableRecord, CallKey, FullValueRecord, Line, PassBy, PathId, Place,
    RValue, StepRecord, TraceLowLevelEvent, VariableId,
};
use codetracer_trace_writer_nim::trace_writer::TraceWriter;
use codetracer_trace_writer_nim::TraceEventsFileFormat;
use pyo3::prelude::*;
use pyo3::types::PyAny;
use recorder_errors::{bug, enverr, target, ErrorCode};
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
        let globals_name = match capture_frame(py, code) {
            Ok(snapshot) => {
                let mapping = snapshot.globals().unwrap_or_else(|| snapshot.locals());
                mapping
                    .get_item("__name__")
                    .ok()
                    .flatten()
                    .and_then(|value| value.extract::<String>().ok())
                    .map(|name| name.trim().to_string())
                    .filter(|name| !name.is_empty())
            }
            Err(_) => None,
        };
        self.filter.set_module_name_hint(code.id(), globals_name);

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

        let scope_resolution = self.filter.cached_resolution(py, code);
        let value_policy = scope_resolution.as_ref().map(|res| res.value_policy());
        let wants_telemetry = value_policy.is_some();

        let mut telemetry_holder = if wants_telemetry {
            Some(self.filter.values_mut())
        } else {
            None
        };
        let telemetry = telemetry_holder.as_deref_mut();
        match capture_call_arguments(py, &mut *self.writer, code, value_policy, telemetry) {
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

        let scope_resolution = self.filter.cached_resolution(py, code);
        let value_policy = scope_resolution.as_ref().map(|res| res.value_policy());
        let wants_telemetry = value_policy.is_some();

        let line_value = Line(lineno as i64);
        let mut recorded_path: Option<(PathId, Line)> = None;

        // M15: derive the column for the upcoming Step from the bytecode
        // line-table. The table is cached per code object so this is O(1)
        // on the steady-state hot path.
        let column_for_step: Option<Line> = self
            .assignment_reconstructor
            .table_for(py, code)
            .ok()
            .and_then(|t| t.first_column_for_line(lineno))
            .map(|c| Line(c as i64));

        let snapshot = capture_frame(py, code)?;
        let frame_raw = snapshot.frame_ptr() as usize as u64;

        // M15: emit Assignment / BindVariable events for every line that
        // has executed since the previous on_line callback in this frame.
        //
        // Rationale: Python's sys.monitoring LINE event fires before the
        // line executes, so at on_line(N) the frame's locals reflect the
        // post-state of line N-1 (the previous on_line callback's line N-1
        // has now completed). For real sys.monitoring this collapses to
        // emitting Assignment events for the single line `prev_line` ==
        // `N-1`. The test harness / pure-Python recorder shim drives
        // on_line less frequently (sometimes only once per script via the
        // `snapshot()` helper), in which case the range of "lines that
        // have executed since the last callback" can span the whole body.
        //
        // The cached `LineAssignmentTable` keys by line number, so we walk
        // the executed range and emit one batch per line that has STOREs
        // in the bytecode. Calls invoked during those lines have already
        // incremented `last_call_key` by this point, so `FunctionReturn
        // { call_key }` references the right CallRecord.
        let previous_line = self.last_line_per_frame.get(&frame_raw).copied();
        let first_to_emit = previous_line.map(|p| p + 1).unwrap_or(0);
        let last_to_emit = lineno.saturating_sub(1);
        if first_to_emit <= last_to_emit {
            if let Ok(table) = self.assignment_reconstructor.table_for(py, code) {
                for line in first_to_emit..=last_to_emit {
                    let assignments = table.for_line(line);
                    if !assignments.is_empty() {
                        emit_assignment_events(
                            &mut *self.writer,
                            &mut self.frame_bound_names,
                            frame_raw,
                            assignments,
                            self.last_call_key,
                            value_policy,
                        );
                    }
                }
            }
        }

        if let Ok(filename) = code.filename(py) {
            let path = Path::new(filename);
            let path_id = TraceWriter::ensure_path_id(&mut *self.writer, path);
            // M15: emit the column-bearing Step variant directly via
            // `add_event` so the NonStreamingTraceWriter / CTFS reader sees
            // the column. `register_step_with_column` falls back to a
            // column-less Step on legacy single-stream Nim writers.
            if column_for_step.is_some() {
                TraceWriter::add_event(
                    &mut *self.writer,
                    TraceLowLevelEvent::Step(StepRecord {
                        path_id,
                        line: line_value,
                        column: column_for_step,
                    }),
                );
            } else {
                TraceWriter::register_step(&mut *self.writer, path, line_value);
            }
            self.mark_event();
            recorded_path = Some((path_id, line_value));
        }

        if let Some((path_id, line)) = recorded_path {
            let frame_id = FrameId::from_raw(snapshot.frame_ptr() as usize as u64);
            self.io
                .record_snapshot(thread::current().id(), path_id, line, frame_id);
        }

        // Remember this line so the next on_line in the same frame can
        // emit Assignment events for it.
        self.last_line_per_frame.insert(frame_raw, lineno);

        let mut recorded: HashSet<String> = HashSet::new();
        let mut telemetry_holder = if wants_telemetry {
            Some(self.filter.values_mut())
        } else {
            None
        };
        let telemetry = telemetry_holder.as_deref_mut();
        record_visible_scope_streaming(
            py,
            &mut *self.writer,
            &mut self.streaming_encoder,
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

        let scope_resolution = self.filter.cached_resolution(py, code);
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
            &mut *self.writer,
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

    fn set_exit_status(&mut self, _py: Python<'_>, exit_code: Option<i32>) -> PyResult<()> {
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
                TraceWriter::finish_writing_trace_events(&mut *self.writer).map_err(|err| {
                    ffi::map_recorder_error(
                        enverr!(ErrorCode::Io, "failed to finalise trace events")
                            .with_context("source", err.to_string()),
                    )
                })?;
            }
            TraceEventsFileFormat::Binary | TraceEventsFileFormat::Ctfs => {
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

        if self.io.teardown(py, &mut *self.writer) {
            self.mark_event();
        }

        self.emit_session_exit(py);

        let exit_summary = self.exit_summary();

        if self.lifecycle.encountered_failure() {
            if policy.keep_partial_trace {
                if let Err(err) =
                    self.lifecycle
                        .finalise(&mut *self.writer, &self.filter, &exit_summary)
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
            self.io.clear_snapshots();
            self.filter.reset();
            self.lifecycle.reset_event_state();
            return Ok(());
        }

        self.lifecycle
            .require_trace_or_fail(&policy)
            .map_err(ffi::map_recorder_error)?;
        self.lifecycle
            .finalise(&mut *self.writer, &self.filter, &exit_summary)
            .map_err(ffi::map_recorder_error)?;
        self.function_ids.clear();
        self.filter.reset();
        self.io.clear_snapshots();
        self.lifecycle.reset_event_state();
        Ok(())
    }
}

/// Encode the M15 Assignment / BindVariable event pair for each store
/// classified by the bytecode reconstructor.
///
/// `frame_bound_names[frame_id]` records the set of names already bound in
/// this frame so we emit `BindVariable` exactly once per name (matching the
/// abstract trace-writer's `bind_variable` semantics in
/// `codetracer_trace_writer::AbstractTraceWriter`).
///
/// `latest_call_key` is the writer-side index of the most recently registered
/// `CallRecord`. When the bytecode classifier returned `FunctionReturn`, we
/// stamp this onto the `RValue::FunctionReturn { call_key }`. If no call has
/// been recorded yet (call key < 0), we degrade to `RValue::Compound(vec![])`
/// rather than emit a dangling reference.
///
/// `policy` (when set) gates emission per target name so the trace filter's
/// value-drop directive also suppresses the corresponding Assignment /
/// BindVariable events — otherwise the reconstructor would leak variable
/// identities for names whose values the filter explicitly drops.
fn emit_assignment_events(
    writer: &mut dyn TraceWriter,
    frame_bound_names: &mut std::collections::HashMap<u64, HashSet<String>>,
    frame_id: u64,
    assignments: &[LineAssignment],
    latest_call_key: i64,
    policy: Option<&ValuePolicy>,
) {
    if assignments.is_empty() {
        return;
    }
    let bound = frame_bound_names.entry(frame_id).or_default();
    for assignment in assignments {
        // Trace-filter integration: if the per-target value policy says
        // "drop", we skip both BindVariable and Assignment for this name
        // so the reconstructor cannot inadvertently leak identifier
        // metadata for filtered values.
        if let Some(p) = policy {
            if matches!(
                p.decide(ValueKind::Local, &assignment.target),
                ValueAction::Drop
            ) {
                continue;
            }
        }
        // Resolve / allocate variable ids by emitting the necessary
        // VariableName events first (the NonStreamingTraceWriter mints ids
        // lazily; `ensure_variable_id` is the canonical entry point).
        let target_id = TraceWriter::ensure_variable_id(writer, &assignment.target);
        let rvalue = build_rvalue(writer, &assignment.rvalue, latest_call_key);

        // BindVariable on first observation of the name in this frame.
        if bound.insert(assignment.target.clone()) {
            TraceWriter::add_event(
                writer,
                TraceLowLevelEvent::BindVariable(BindVariableRecord {
                    variable_id: target_id,
                    place: Place(0),
                }),
            );
        }
        TraceWriter::add_event(
            writer,
            TraceLowLevelEvent::Assignment(AssignmentRecord {
                to: target_id,
                pass_by: PassBy::Value,
                from: rvalue,
            }),
        );
    }
}

fn build_rvalue(writer: &mut dyn TraceWriter, shape: &RValueShape, latest_call_key: i64) -> RValue {
    match shape {
        RValueShape::Literal => RValue::Literal,
        RValueShape::Simple { source } => {
            let id = TraceWriter::ensure_variable_id(writer, source);
            RValue::Simple(id)
        }
        RValueShape::FieldAccess { receiver, field } => {
            let id = TraceWriter::ensure_variable_id(writer, receiver);
            RValue::FieldAccess {
                receiver: id,
                field: field.clone(),
            }
        }
        RValueShape::IndexAccess { receiver, index } => {
            let id = TraceWriter::ensure_variable_id(writer, receiver);
            RValue::IndexAccess {
                receiver: id,
                index: *index,
            }
        }
        RValueShape::FunctionReturn => {
            // The call key must reference a previously recorded
            // CallRecord. If we have not seen one yet (e.g. the call landed
            // before the recorder activated) we fall back to a Compound
            // marker so the decoder is never asked to dereference an
            // invalid CallKey.
            if latest_call_key >= 0 {
                RValue::FunctionReturn {
                    call_key: CallKey(latest_call_key),
                }
            } else {
                RValue::Compound(vec![])
            }
        }
        RValueShape::Compound { sources } => {
            let ids: Vec<VariableId> = sources
                .iter()
                .map(|s| TraceWriter::ensure_variable_id(writer, s))
                .collect();
            RValue::Compound(ids)
        }
        RValueShape::Unknown => RValue::Compound(vec![]),
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
            TraceWriter::register_call(&mut *self.writer, fid, args);
            // M15: the writer's CallRecord index advances by exactly one per
            // register_call call. Track that so we can stamp the next
            // observed `result = foo()` assignment with the matching CallKey.
            self.last_call_key += 1;
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

        let scope_resolution = self.filter.cached_resolution(py, code);
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

        record_return_value_streaming(
            py,
            &mut *self.writer,
            &mut self.streaming_encoder,
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
