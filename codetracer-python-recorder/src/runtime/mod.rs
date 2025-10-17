//! Runtime tracer facade translating sys.monitoring callbacks into `runtime_tracing` records.

mod activation;
mod frame_inspector;
pub mod io_capture;
mod line_snapshots;
mod logging;
mod output_paths;
mod value_capture;
mod value_encoder;

pub use line_snapshots::{FrameId, LineSnapshotStore};
pub use output_paths::TraceOutputPaths;

use activation::ActivationController;
use frame_inspector::capture_frame;
use logging::log_event;
use value_capture::{
    capture_call_arguments, record_return_value, record_visible_scope, ValueRedactionStats,
};

use std::collections::{hash_map::Entry, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(feature = "integration-test")]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
#[cfg(feature = "integration-test")]
use std::sync::OnceLock;
use std::thread::{self, ThreadId};

use pyo3::prelude::*;
use pyo3::types::PyAny;

use recorder_errors::{bug, enverr, target, usage, ErrorCode, RecorderResult};
use runtime_tracing::NonStreamingTraceWriter;
use runtime_tracing::{
    EventLogKind, Line, PathId, RecordEvent, TraceEventsFileFormat, TraceLowLevelEvent, TraceWriter,
};

use crate::code_object::CodeObjectWrapper;
use crate::ffi;
use crate::logging::{record_dropped_event, set_active_trace_id, with_error_code};
use crate::monitoring::{
    events_union, CallbackOutcome, CallbackResult, EventSet, MonitoringEvents, Tracer,
};
use crate::policy::{policy_snapshot, RecorderPolicy};
use crate::runtime::io_capture::{
    IoCapturePipeline, IoCaptureSettings, IoChunk, IoChunkFlags, IoStream, ScopedMuteIoCapture,
};
use crate::trace_filter::engine::{ExecDecision, ScopeResolution, TraceFilterEngine, ValueKind};
use serde::Serialize;
use serde_json::{self, json};

use uuid::Uuid;

struct TraceIdResetGuard;

impl TraceIdResetGuard {
    fn new() -> Self {
        TraceIdResetGuard
    }
}

impl Drop for TraceIdResetGuard {
    fn drop(&mut self) {
        set_active_trace_id(None);
    }
}

fn io_flag_labels(flags: IoChunkFlags) -> Vec<&'static str> {
    let mut labels = Vec::new();
    if flags.contains(IoChunkFlags::NEWLINE_TERMINATED) {
        labels.push("newline");
    }
    if flags.contains(IoChunkFlags::EXPLICIT_FLUSH) {
        labels.push("flush");
    }
    if flags.contains(IoChunkFlags::STEP_BOUNDARY) {
        labels.push("step_boundary");
    }
    if flags.contains(IoChunkFlags::TIME_SPLIT) {
        labels.push("time_split");
    }
    if flags.contains(IoChunkFlags::INPUT_CHUNK) {
        labels.push("input");
    }
    if flags.contains(IoChunkFlags::FD_MIRROR) {
        labels.push("mirror");
    }
    labels
}

/// Minimal runtime tracer that maps Python sys.monitoring events to
/// runtime_tracing writer operations.
pub struct RuntimeTracer {
    writer: NonStreamingTraceWriter,
    format: TraceEventsFileFormat,
    activation: ActivationController,
    program_path: PathBuf,
    ignored_code_ids: HashSet<usize>,
    function_ids: HashMap<usize, runtime_tracing::FunctionId>,
    output_paths: Option<TraceOutputPaths>,
    events_recorded: bool,
    encountered_failure: bool,
    trace_id: String,
    line_snapshots: Arc<LineSnapshotStore>,
    io_capture: Option<IoCapturePipeline>,
    trace_filter: Option<Arc<TraceFilterEngine>>,
    scope_cache: HashMap<usize, Arc<ScopeResolution>>,
    filter_stats: FilterStats,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShouldTrace {
    Trace,
    SkipAndDisable,
}

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

#[derive(Debug, Default)]
struct FilterStats {
    skipped_scopes: u64,
    redactions: ValueRedactionStats,
}

impl FilterStats {
    fn record_skip(&mut self) {
        self.skipped_scopes += 1;
    }

    fn redactions_mut(&mut self) -> &mut ValueRedactionStats {
        &mut self.redactions
    }

    fn reset(&mut self) {
        self.skipped_scopes = 0;
        self.redactions = ValueRedactionStats::default();
    }

    fn summary_json(&self) -> serde_json::Value {
        let mut redactions = serde_json::Map::new();
        for kind in ValueKind::ALL {
            redactions.insert(kind.label().to_string(), json!(self.redactions.count(kind)));
        }
        json!({
            "scopes_skipped": self.skipped_scopes,
            "value_redactions": serde_json::Value::Object(redactions),
        })
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
static FAILURE_MODE: OnceLock<Option<FailureMode>> = OnceLock::new();
#[cfg(feature = "integration-test")]
static FAILURE_TRIGGERED: AtomicBool = AtomicBool::new(false);

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
fn suppress_events() -> bool {
    matches!(configured_failure_mode(), Some(FailureMode::SuppressEvents))
}

#[cfg(not(feature = "integration-test"))]
fn suppress_events() -> bool {
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

fn is_real_filename(filename: &str) -> bool {
    let trimmed = filename.trim();
    !(trimmed.starts_with('<') && trimmed.ends_with('>'))
}

impl RuntimeTracer {
    pub fn new(
        program: &str,
        args: &[String],
        format: TraceEventsFileFormat,
        activation_path: Option<&Path>,
        trace_filter: Option<Arc<TraceFilterEngine>>,
    ) -> Self {
        let mut writer = NonStreamingTraceWriter::new(program, args);
        writer.set_format(format);
        let activation = ActivationController::new(activation_path);
        let program_path = PathBuf::from(program);
        Self {
            writer,
            format,
            activation,
            program_path,
            ignored_code_ids: HashSet::new(),
            function_ids: HashMap::new(),
            output_paths: None,
            events_recorded: false,
            encountered_failure: false,
            trace_id: Uuid::new_v4().to_string(),
            line_snapshots: Arc::new(LineSnapshotStore::new()),
            io_capture: None,
            trace_filter,
            scope_cache: HashMap::new(),
            filter_stats: FilterStats::default(),
        }
    }

    /// Share the snapshot store with collaborators (IO capture, tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn line_snapshot_store(&self) -> Arc<LineSnapshotStore> {
        Arc::clone(&self.line_snapshots)
    }

    pub fn install_io_capture(&mut self, py: Python<'_>, policy: &RecorderPolicy) -> PyResult<()> {
        let settings = IoCaptureSettings {
            line_proxies: policy.io_capture.line_proxies,
            fd_mirror: policy.io_capture.fd_fallback,
        };
        let pipeline = IoCapturePipeline::install(py, Arc::clone(&self.line_snapshots), settings)?;
        self.io_capture = pipeline;
        Ok(())
    }

    fn flush_io_before_step(&mut self, thread_id: ThreadId) {
        if let Some(pipeline) = self.io_capture.as_ref() {
            pipeline.flush_before_step(thread_id);
        }
        self.drain_io_chunks();
    }

    fn flush_pending_io(&mut self) {
        if let Some(pipeline) = self.io_capture.as_ref() {
            pipeline.flush_all();
        }
        self.drain_io_chunks();
    }

    fn drain_io_chunks(&mut self) {
        if let Some(pipeline) = self.io_capture.as_ref() {
            let chunks = pipeline.drain_chunks();
            for chunk in chunks {
                self.record_io_chunk(chunk);
            }
        }
    }

    fn record_io_chunk(&mut self, mut chunk: IoChunk) {
        if chunk.path_id.is_none() {
            if let Some(path) = chunk.path.as_deref() {
                let path_id = TraceWriter::ensure_path_id(&mut self.writer, Path::new(path));
                chunk.path_id = Some(path_id);
            }
        }

        let kind = match chunk.stream {
            IoStream::Stdout => EventLogKind::Write,
            IoStream::Stderr => EventLogKind::WriteOther,
            IoStream::Stdin => EventLogKind::Read,
        };

        let metadata = self.build_io_metadata(&chunk);
        let content = String::from_utf8_lossy(&chunk.payload).into_owned();

        TraceWriter::add_event(
            &mut self.writer,
            TraceLowLevelEvent::Event(RecordEvent {
                kind,
                metadata,
                content,
            }),
        );
        self.mark_event();
    }

    fn scope_resolution(
        &mut self,
        py: Python<'_>,
        code: &CodeObjectWrapper,
    ) -> Option<Arc<ScopeResolution>> {
        let engine = self.trace_filter.as_ref()?;
        let code_id = code.id();

        if let Some(existing) = self.scope_cache.get(&code_id) {
            return Some(existing.clone());
        }

        match engine.resolve(py, code) {
            Ok(resolution) => {
                if resolution.exec() == ExecDecision::Trace {
                    self.scope_cache.insert(code_id, Arc::clone(&resolution));
                } else {
                    self.scope_cache.remove(&code_id);
                }
                Some(resolution)
            }
            Err(err) => {
                let message = err.to_string();
                let error_code = err.code;
                with_error_code(error_code, || {
                    let _mute = ScopedMuteIoCapture::new();
                    log::error!(
                        "[RuntimeTracer] trace filter resolution failed for code id {}: {}",
                        code_id,
                        message
                    );
                });
                record_dropped_event("filter_resolution_error");
                None
            }
        }
    }

    fn build_io_metadata(&self, chunk: &IoChunk) -> String {
        #[derive(Serialize)]
        struct IoEventMetadata<'a> {
            stream: &'a str,
            thread: String,
            path_id: Option<usize>,
            line: Option<i64>,
            frame_id: Option<u64>,
            flags: Vec<&'a str>,
        }

        let snapshot = self.line_snapshots.snapshot_for_thread(chunk.thread_id);
        let path_id = chunk
            .path_id
            .map(|id| id.0)
            .or_else(|| snapshot.as_ref().map(|snap| snap.path_id().0));
        let line = chunk
            .line
            .map(|line| line.0)
            .or_else(|| snapshot.as_ref().map(|snap| snap.line().0));
        let frame_id = chunk
            .frame_id
            .or_else(|| snapshot.as_ref().map(|snap| snap.frame_id()));

        let metadata = IoEventMetadata {
            stream: match chunk.stream {
                IoStream::Stdout => "stdout",
                IoStream::Stderr => "stderr",
                IoStream::Stdin => "stdin",
            },
            thread: format!("{:?}", chunk.thread_id),
            path_id,
            line,
            frame_id: frame_id.map(|id| id.as_raw()),
            flags: io_flag_labels(chunk.flags),
        };

        match serde_json::to_string(&metadata) {
            Ok(json) => json,
            Err(err) => {
                let _mute = ScopedMuteIoCapture::new();
                log::error!("failed to serialise IO metadata: {err}");
                "{}".to_string()
            }
        }
    }

    fn teardown_io_capture(&mut self, py: Python<'_>) {
        if let Some(mut pipeline) = self.io_capture.take() {
            pipeline.flush_all();
            let chunks = pipeline.drain_chunks();
            for chunk in chunks {
                self.record_io_chunk(chunk);
            }
            pipeline.uninstall(py);
            let trailing = pipeline.drain_chunks();
            for chunk in trailing {
                self.record_io_chunk(chunk);
            }
        }
    }

    /// Configure output files and write initial metadata records.
    pub fn begin(&mut self, outputs: &TraceOutputPaths, start_line: u32) -> PyResult<()> {
        let start_path = self.activation.start_path(&self.program_path);
        {
            let _mute = ScopedMuteIoCapture::new();
            log::debug!("{}", start_path.display());
        }
        outputs
            .configure_writer(&mut self.writer, start_path, start_line)
            .map_err(ffi::map_recorder_error)?;
        self.output_paths = Some(outputs.clone());
        self.events_recorded = false;
        self.encountered_failure = false;
        set_active_trace_id(Some(self.trace_id.clone()));
        Ok(())
    }

    fn mark_event(&mut self) {
        if suppress_events() {
            let _mute = ScopedMuteIoCapture::new();
            log::debug!("[RuntimeTracer] skipping event mark due to test injection");
            return;
        }
        self.events_recorded = true;
    }

    fn mark_failure(&mut self) {
        self.encountered_failure = true;
    }

    fn cleanup_partial_outputs(&self) -> RecorderResult<()> {
        if let Some(outputs) = &self.output_paths {
            for path in [outputs.events(), outputs.metadata(), outputs.paths()] {
                if path.exists() {
                    fs::remove_file(path).map_err(|err| {
                        enverr!(ErrorCode::Io, "failed to remove partial trace file")
                            .with_context("path", path.display().to_string())
                            .with_context("io", err.to_string())
                    })?;
                }
            }
        }
        Ok(())
    }

    fn require_trace_or_fail(&self, policy: &RecorderPolicy) -> RecorderResult<()> {
        if policy.require_trace && !self.events_recorded {
            return Err(usage!(
                ErrorCode::TraceMissing,
                "recorder policy requires a trace but no events were recorded"
            ));
        }
        Ok(())
    }

    fn finalise_writer(&mut self) -> RecorderResult<()> {
        TraceWriter::finish_writing_trace_metadata(&mut self.writer).map_err(|err| {
            enverr!(ErrorCode::Io, "failed to finalise trace metadata")
                .with_context("source", err.to_string())
        })?;
        self.append_filter_metadata()?;
        TraceWriter::finish_writing_trace_paths(&mut self.writer).map_err(|err| {
            enverr!(ErrorCode::Io, "failed to finalise trace paths")
                .with_context("source", err.to_string())
        })?;
        TraceWriter::finish_writing_trace_events(&mut self.writer).map_err(|err| {
            enverr!(ErrorCode::Io, "failed to finalise trace events")
                .with_context("source", err.to_string())
        })?;
        Ok(())
    }

    fn append_filter_metadata(&self) -> RecorderResult<()> {
        let Some(outputs) = &self.output_paths else {
            return Ok(());
        };
        let Some(engine) = self.trace_filter.as_ref() else {
            return Ok(());
        };

        let path = outputs.metadata();
        let original = fs::read_to_string(path).map_err(|err| {
            enverr!(ErrorCode::Io, "failed to read trace metadata")
                .with_context("path", path.display().to_string())
                .with_context("source", err.to_string())
        })?;

        let mut metadata: serde_json::Value = serde_json::from_str(&original).map_err(|err| {
            enverr!(ErrorCode::Io, "failed to parse trace metadata JSON")
                .with_context("path", path.display().to_string())
                .with_context("source", err.to_string())
        })?;

        let filters = engine.summary();
        let filters_json: Vec<serde_json::Value> = filters
            .entries
            .iter()
            .map(|entry| {
                json!({
                    "path": entry.path.to_string_lossy(),
                    "sha256": entry.sha256,
                    "name": entry.name,
                    "version": entry.version,
                })
            })
            .collect();

        if let serde_json::Value::Object(ref mut obj) = metadata {
            obj.insert(
                "trace_filter".to_string(),
                json!({
                    "filters": filters_json,
                    "stats": self.filter_stats.summary_json(),
                }),
            );
            let serialised = serde_json::to_string(&metadata).map_err(|err| {
                enverr!(ErrorCode::Io, "failed to serialise trace metadata")
                    .with_context("path", path.display().to_string())
                    .with_context("source", err.to_string())
            })?;
            fs::write(path, serialised).map_err(|err| {
                enverr!(ErrorCode::Io, "failed to write trace metadata")
                    .with_context("path", path.display().to_string())
                    .with_context("source", err.to_string())
            })?;
            Ok(())
        } else {
            Err(
                enverr!(ErrorCode::Io, "trace metadata must be a JSON object")
                    .with_context("path", path.display().to_string()),
            )
        }
    }

    fn ensure_function_id(
        &mut self,
        py: Python<'_>,
        code: &CodeObjectWrapper,
    ) -> PyResult<runtime_tracing::FunctionId> {
        match self.function_ids.entry(code.id()) {
            Entry::Occupied(entry) => Ok(*entry.get()),
            Entry::Vacant(slot) => {
                let name = code.qualname(py)?;
                let filename = code.filename(py)?;
                let first_line = code.first_line(py)?;
                let function_id = TraceWriter::ensure_function_id(
                    &mut self.writer,
                    name,
                    Path::new(filename),
                    Line(first_line as i64),
                );
                Ok(*slot.insert(function_id))
            }
        }
    }

    fn should_trace_code(&mut self, py: Python<'_>, code: &CodeObjectWrapper) -> ShouldTrace {
        let code_id = code.id();
        if self.ignored_code_ids.contains(&code_id) {
            return ShouldTrace::SkipAndDisable;
        }

        if let Some(resolution) = self.scope_resolution(py, code) {
            match resolution.exec() {
                ExecDecision::Skip => {
                    self.scope_cache.remove(&code_id);
                    self.filter_stats.record_skip();
                    self.ignored_code_ids.insert(code_id);
                    record_dropped_event("filter_scope_skip");
                    return ShouldTrace::SkipAndDisable;
                }
                ExecDecision::Trace => {
                    // already cached for future use
                }
            }
        }

        let filename = match code.filename(py) {
            Ok(name) => name,
            Err(err) => {
                with_error_code(ErrorCode::Io, || {
                    let _mute = ScopedMuteIoCapture::new();
                    log::error!("failed to resolve code filename: {err}");
                });
                record_dropped_event("filename_lookup_failed");
                self.scope_cache.remove(&code_id);
                self.ignored_code_ids.insert(code_id);
                return ShouldTrace::SkipAndDisable;
            }
        };
        if is_real_filename(filename) {
            ShouldTrace::Trace
        } else {
            self.scope_cache.remove(&code_id);
            self.ignored_code_ids.insert(code_id);
            record_dropped_event("synthetic_filename");
            ShouldTrace::SkipAndDisable
        }
    }
}

impl Tracer for RuntimeTracer {
    fn interest(&self, events: &MonitoringEvents) -> EventSet {
        // Minimal set: function start, step lines, and returns
        events_union(&[events.PY_START, events.LINE, events.PY_RETURN])
    }

    fn on_py_start(
        &mut self,
        py: Python<'_>,
        code: &CodeObjectWrapper,
        _offset: i32,
    ) -> CallbackResult {
        let is_active = self.activation.should_process_event(py, code);
        if matches!(
            self.should_trace_code(py, code),
            ShouldTrace::SkipAndDisable
        ) {
            return Ok(CallbackOutcome::DisableLocation);
        }
        if !is_active {
            return Ok(CallbackOutcome::Continue);
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

        let scope_resolution = self.scope_cache.get(&code.id()).cloned();
        let value_policy = scope_resolution.as_ref().map(|res| res.value_policy());
        let wants_telemetry = value_policy.is_some();

        if let Ok(fid) = self.ensure_function_id(py, code) {
            let mut telemetry_holder = if wants_telemetry {
                Some(self.filter_stats.redactions_mut())
            } else {
                None
            };
            let telemetry = telemetry_holder.as_deref_mut();
            match capture_call_arguments(py, &mut self.writer, code, value_policy, telemetry) {
                Ok(args) => TraceWriter::register_call(&mut self.writer, fid, args),
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
            self.mark_event();
        }

        Ok(CallbackOutcome::Continue)
    }

    fn on_line(&mut self, py: Python<'_>, code: &CodeObjectWrapper, lineno: u32) -> CallbackResult {
        let is_active = self.activation.should_process_event(py, code);
        if matches!(
            self.should_trace_code(py, code),
            ShouldTrace::SkipAndDisable
        ) {
            return Ok(CallbackOutcome::DisableLocation);
        }
        if !is_active {
            return Ok(CallbackOutcome::Continue);
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

        let scope_resolution = self.scope_cache.get(&code.id()).cloned();
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
            self.line_snapshots
                .record(thread::current().id(), path_id, line, frame_id);
        }

        let mut recorded: HashSet<String> = HashSet::new();
        let mut telemetry_holder = if wants_telemetry {
            Some(self.filter_stats.redactions_mut())
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
        let is_active = self.activation.should_process_event(py, code);
        if matches!(
            self.should_trace_code(py, code),
            ShouldTrace::SkipAndDisable
        ) {
            return Ok(CallbackOutcome::DisableLocation);
        }
        if !is_active {
            return Ok(CallbackOutcome::Continue);
        }

        log_event(py, code, "on_py_return", None);

        self.flush_pending_io();

        let scope_resolution = self.scope_cache.get(&code.id()).cloned();
        let value_policy = scope_resolution.as_ref().map(|res| res.value_policy());
        let wants_telemetry = value_policy.is_some();
        let object_name = scope_resolution.as_ref().and_then(|res| res.object_name());

        let mut telemetry_holder = if wants_telemetry {
            Some(self.filter_stats.redactions_mut())
        } else {
            None
        };
        let telemetry = telemetry_holder.as_deref_mut();

        record_return_value(
            py,
            &mut self.writer,
            retval,
            value_policy,
            telemetry,
            object_name,
        );
        self.mark_event();
        if self.activation.handle_return_event(code.id()) {
            let _mute = ScopedMuteIoCapture::new();
            log::debug!("[RuntimeTracer] deactivated on activation return");
        }

        Ok(CallbackOutcome::Continue)
    }

    fn notify_failure(&mut self, _py: Python<'_>) -> PyResult<()> {
        self.mark_failure();
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
        self.ignored_code_ids.clear();
        self.scope_cache.clear();
        Ok(())
    }

    fn finish(&mut self, py: Python<'_>) -> PyResult<()> {
        // Trace event entry
        let _mute_finish = ScopedMuteIoCapture::new();
        log::debug!("[RuntimeTracer] finish");

        if should_inject_failure(FailureStage::Finish) {
            return Err(injected_failure_err(FailureStage::Finish));
        }

        set_active_trace_id(Some(self.trace_id.clone()));
        let _reset = TraceIdResetGuard::new();
        let policy = policy_snapshot();

        self.teardown_io_capture(py);

        if self.encountered_failure {
            if policy.keep_partial_trace {
                if let Err(err) = self.finalise_writer() {
                    with_error_code(ErrorCode::TraceIncomplete, || {
                        log::warn!(
                            "failed to finalise partial trace after disable: {}",
                            err.message()
                        );
                    });
                }
                if let Some(outputs) = &self.output_paths {
                    with_error_code(ErrorCode::TraceIncomplete, || {
                        log::warn!(
                            "recorder detached after failure; keeping partial trace at {}",
                            outputs.events().display()
                        );
                    });
                }
            } else {
                self.cleanup_partial_outputs()
                    .map_err(ffi::map_recorder_error)?;
            }
            self.ignored_code_ids.clear();
            self.function_ids.clear();
            self.scope_cache.clear();
            self.line_snapshots.clear();
            self.filter_stats.reset();
            return Ok(());
        }

        self.require_trace_or_fail(&policy)
            .map_err(ffi::map_recorder_error)?;
        self.finalise_writer().map_err(ffi::map_recorder_error)?;
        self.ignored_code_ids.clear();
        self.function_ids.clear();
        self.scope_cache.clear();
        self.filter_stats.reset();
        self.line_snapshots.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitoring::CallbackOutcome;
    use crate::policy;
    use crate::trace_filter::config::TraceFilterConfig;
    use pyo3::types::{PyAny, PyCode, PyModule};
    use pyo3::wrap_pyfunction;
    use runtime_tracing::{FullValueRecord, StepRecord, TraceLowLevelEvent, ValueRecord};
    use serde::Deserialize;
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use std::ffi::CString;
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;
    use std::thread;

    thread_local! {
        static ACTIVE_TRACER: Cell<*mut RuntimeTracer> = Cell::new(std::ptr::null_mut());
        static LAST_OUTCOME: Cell<Option<CallbackOutcome>> = Cell::new(None);
    }

    struct ScopedTracer;

    impl ScopedTracer {
        fn new(tracer: &mut RuntimeTracer) -> Self {
            let ptr = tracer as *mut _;
            ACTIVE_TRACER.with(|cell| cell.set(ptr));
            ScopedTracer
        }
    }

    impl Drop for ScopedTracer {
        fn drop(&mut self) {
            ACTIVE_TRACER.with(|cell| cell.set(std::ptr::null_mut()));
        }
    }

    fn last_outcome() -> Option<CallbackOutcome> {
        LAST_OUTCOME.with(|cell| cell.get())
    }

    fn reset_policy(_py: Python<'_>) {
        policy::configure_policy_py(
            Some("abort"),
            Some(false),
            Some(false),
            None,
            None,
            Some(false),
            None,
            None,
        )
        .expect("reset recorder policy");
    }

    #[test]
    fn detects_real_filenames() {
        assert!(is_real_filename("example.py"));
        assert!(is_real_filename(" /tmp/module.py "));
        assert!(is_real_filename("src/<tricky>.py"));
        assert!(!is_real_filename("<string>"));
        assert!(!is_real_filename("  <stdin>  "));
        assert!(!is_real_filename("<frozen importlib._bootstrap>"));
    }

    #[test]
    fn skips_synthetic_filename_events() {
        Python::with_gil(|py| {
            let mut tracer =
                RuntimeTracer::new("test.py", &[], TraceEventsFileFormat::Json, None, None);
            ensure_test_module(py);
            let script = format!("{PRELUDE}\nsnapshot()\n");
            {
                let _guard = ScopedTracer::new(&mut tracer);
                LAST_OUTCOME.with(|cell| cell.set(None));
                let script_c = CString::new(script).expect("script contains nul byte");
                py.run(script_c.as_c_str(), None, None)
                    .expect("execute synthetic script");
            }
            assert!(
                tracer.writer.events.is_empty(),
                "expected no events for synthetic filename"
            );
            assert_eq!(last_outcome(), Some(CallbackOutcome::DisableLocation));

            let compile_fn = py
                .import("builtins")
                .expect("import builtins")
                .getattr("compile")
                .expect("fetch compile");
            let binding = compile_fn
                .call1(("pass", "<string>", "exec"))
                .expect("compile code object");
            let code_obj = binding.downcast::<PyCode>().expect("downcast code object");
            let wrapper = CodeObjectWrapper::new(py, &code_obj);
            assert_eq!(
                tracer.should_trace_code(py, &wrapper),
                ShouldTrace::SkipAndDisable
            );
        });
    }

    #[test]
    fn traces_real_file_events() {
        let snapshots = run_traced_script("snapshot()\n");
        assert!(
            !snapshots.is_empty(),
            "expected snapshots for real file execution"
        );
        assert_eq!(last_outcome(), Some(CallbackOutcome::Continue));
    }

    #[test]
    fn callbacks_do_not_import_sys_monitoring() {
        let body = r#"
import builtins
_orig_import = builtins.__import__

def guard(name, *args, **kwargs):
    if name == "sys.monitoring":
        raise RuntimeError("callback imported sys.monitoring")
    return _orig_import(name, *args, **kwargs)

builtins.__import__ = guard
try:
    snapshot()
finally:
    builtins.__import__ = _orig_import
"#;
        let snapshots = run_traced_script(body);
        assert!(
            !snapshots.is_empty(),
            "expected snapshots when import guard active"
        );
        assert_eq!(last_outcome(), Some(CallbackOutcome::Continue));
    }

    #[test]
    fn records_return_values_and_deactivates_activation() {
        Python::with_gil(|py| {
            ensure_test_module(py);
            let tmp = tempfile::tempdir().expect("create temp dir");
            let script_path = tmp.path().join("activation_script.py");
            let script = format!(
                "{PRELUDE}\n\n\
def compute():\n    emit_return(\"tail\")\n    return \"tail\"\n\n\
result = compute()\n"
            );
            std::fs::write(&script_path, &script).expect("write script");

            let program = script_path.to_string_lossy().into_owned();
            let mut tracer = RuntimeTracer::new(
                &program,
                &[],
                TraceEventsFileFormat::Json,
                Some(script_path.as_path()),
                None,
            );

            {
                let _guard = ScopedTracer::new(&mut tracer);
                LAST_OUTCOME.with(|cell| cell.set(None));
                let run_code = format!(
                    "import runpy\nrunpy.run_path(r\"{}\")",
                    script_path.display()
                );
                let run_code_c = CString::new(run_code).expect("script contains nul byte");
                py.run(run_code_c.as_c_str(), None, None)
                    .expect("execute test script");
            }

            let returns: Vec<SimpleValue> = tracer
                .writer
                .events
                .iter()
                .filter_map(|event| match event {
                    TraceLowLevelEvent::Return(record) => {
                        Some(SimpleValue::from_value(&record.return_value))
                    }
                    _ => None,
                })
                .collect();

            assert!(
                returns.contains(&SimpleValue::String("tail".to_string())),
                "expected recorded string return, got {:?}",
                returns
            );
            assert_eq!(last_outcome(), Some(CallbackOutcome::Continue));
            assert!(!tracer.activation.is_active());
        });
    }

    #[test]
    fn line_snapshot_store_tracks_last_step() {
        Python::with_gil(|py| {
            ensure_test_module(py);
            let tmp = tempfile::tempdir().expect("create temp dir");
            let script_path = tmp.path().join("snapshot_script.py");
            let script = format!("{PRELUDE}\n\nsnapshot()\n");
            std::fs::write(&script_path, &script).expect("write script");

            let mut tracer = RuntimeTracer::new(
                "snapshot_script.py",
                &[],
                TraceEventsFileFormat::Json,
                None,
                None,
            );
            let store = tracer.line_snapshot_store();

            {
                let _guard = ScopedTracer::new(&mut tracer);
                LAST_OUTCOME.with(|cell| cell.set(None));
                let run_code = format!(
                    "import runpy\nrunpy.run_path(r\"{}\")",
                    script_path.display()
                );
                let run_code_c = CString::new(run_code).expect("script contains nul byte");
                py.run(run_code_c.as_c_str(), None, None)
                    .expect("execute snapshot script");
            }

            let last_step: StepRecord = tracer
                .writer
                .events
                .iter()
                .rev()
                .find_map(|event| match event {
                    TraceLowLevelEvent::Step(step) => Some(step.clone()),
                    _ => None,
                })
                .expect("expected one step event");

            let thread_id = thread::current().id();
            let snapshot = store
                .snapshot_for_thread(thread_id)
                .expect("snapshot should be recorded");

            assert_eq!(snapshot.line(), last_step.line);
            assert_eq!(snapshot.path_id(), last_step.path_id);
            assert!(snapshot.captured_at().elapsed().as_secs_f64() >= 0.0);
        });
    }

    #[derive(Debug, Deserialize)]
    struct IoMetadata {
        stream: String,
        path_id: Option<usize>,
        line: Option<i64>,
        flags: Vec<String>,
    }

    #[test]
    fn io_capture_records_python_and_native_output() {
        Python::with_gil(|py| {
            reset_policy(py);
            policy::configure_policy_py(
                Some("abort"),
                Some(false),
                Some(false),
                None,
                None,
                Some(false),
                Some(true),
                Some(false),
            )
            .expect("enable io capture proxies");

            ensure_test_module(py);
            let tmp = tempfile::tempdir().expect("create temp dir");
            let script_path = tmp.path().join("io_script.py");
            let script = format!(
                "{PRELUDE}\n\nprint('python out')\nfrom ctypes import pythonapi, c_char_p\npythonapi.PySys_WriteStdout(c_char_p(b'native out\\n'))\n"
            );
            std::fs::write(&script_path, &script).expect("write script");

            let mut tracer = RuntimeTracer::new(
                script_path.to_string_lossy().as_ref(),
                &[],
                TraceEventsFileFormat::Json,
                None,
                None,
            );
            let outputs = TraceOutputPaths::new(tmp.path(), TraceEventsFileFormat::Json);
            tracer.begin(&outputs, 1).expect("begin tracer");
            tracer
                .install_io_capture(py, &policy::policy_snapshot())
                .expect("install io capture");

            {
                let _guard = ScopedTracer::new(&mut tracer);
                LAST_OUTCOME.with(|cell| cell.set(None));
                let run_code = format!(
                    "import runpy\nrunpy.run_path(r\"{}\")",
                    script_path.display()
                );
                let run_code_c = CString::new(run_code).expect("script contains nul byte");
                py.run(run_code_c.as_c_str(), None, None)
                    .expect("execute io script");
            }

            tracer.finish(py).expect("finish tracer");

            let io_events: Vec<(IoMetadata, Vec<u8>)> = tracer
                .writer
                .events
                .iter()
                .filter_map(|event| match event {
                    TraceLowLevelEvent::Event(record) => {
                        let metadata: IoMetadata = serde_json::from_str(&record.metadata).ok()?;
                        Some((metadata, record.content.as_bytes().to_vec()))
                    }
                    _ => None,
                })
                .collect();

            assert!(io_events
                .iter()
                .any(|(meta, payload)| meta.stream == "stdout"
                    && String::from_utf8_lossy(payload).contains("python out")));
            assert!(io_events
                .iter()
                .any(|(meta, payload)| meta.stream == "stdout"
                    && String::from_utf8_lossy(payload).contains("native out")));
            assert!(io_events.iter().all(|(meta, _)| {
                if meta.stream == "stdout" {
                    meta.path_id.is_some() && meta.line.is_some()
                } else {
                    true
                }
            }));
            assert!(io_events
                .iter()
                .filter(|(meta, _)| meta.stream == "stdout")
                .any(|(meta, _)| meta.flags.iter().any(|flag| flag == "newline")));

            reset_policy(py);
        });
    }

    #[cfg(unix)]
    #[test]
    fn fd_mirror_captures_os_write_payloads() {
        Python::with_gil(|py| {
            reset_policy(py);
            policy::configure_policy_py(
                Some("abort"),
                Some(false),
                Some(false),
                None,
                None,
                Some(false),
                Some(true),
                Some(true),
            )
            .expect("enable io capture with fd fallback");

            ensure_test_module(py);
            let tmp = tempfile::tempdir().expect("tempdir");
            let script_path = tmp.path().join("fd_mirror.py");
            std::fs::write(
                &script_path,
                format!(
                    "{PRELUDE}\nimport os\nprint('proxy line')\nos.write(1, b'fd stdout\\n')\nos.write(2, b'fd stderr\\n')\n"
                ),
            )
            .expect("write script");

            let mut tracer = RuntimeTracer::new(
                script_path.to_string_lossy().as_ref(),
                &[],
                TraceEventsFileFormat::Json,
                None,
                None,
            );
            let outputs = TraceOutputPaths::new(tmp.path(), TraceEventsFileFormat::Json);
            tracer.begin(&outputs, 1).expect("begin tracer");
            tracer
                .install_io_capture(py, &policy::policy_snapshot())
                .expect("install io capture");

            {
                let _guard = ScopedTracer::new(&mut tracer);
                LAST_OUTCOME.with(|cell| cell.set(None));
                let run_code = format!(
                    "import runpy\nrunpy.run_path(r\"{}\")",
                    script_path.display()
                );
                let run_code_c = CString::new(run_code).expect("script contains nul byte");
                py.run(run_code_c.as_c_str(), None, None)
                    .expect("execute fd script");
            }

            tracer.finish(py).expect("finish tracer");

            let io_events: Vec<(IoMetadata, Vec<u8>)> = tracer
                .writer
                .events
                .iter()
                .filter_map(|event| match event {
                    TraceLowLevelEvent::Event(record) => {
                        let metadata: IoMetadata = serde_json::from_str(&record.metadata).ok()?;
                        Some((metadata, record.content.as_bytes().to_vec()))
                    }
                    _ => None,
                })
                .collect();

            let stdout_mirror = io_events.iter().find(|(meta, _)| {
                meta.stream == "stdout" && meta.flags.iter().any(|flag| flag == "mirror")
            });
            assert!(
                stdout_mirror.is_some(),
                "expected mirror event for stdout: {:?}",
                io_events
            );
            let stdout_payload = &stdout_mirror.unwrap().1;
            assert!(
                String::from_utf8_lossy(stdout_payload).contains("fd stdout"),
                "mirror stdout payload missing expected text"
            );

            let stderr_mirror = io_events.iter().find(|(meta, _)| {
                meta.stream == "stderr" && meta.flags.iter().any(|flag| flag == "mirror")
            });
            assert!(
                stderr_mirror.is_some(),
                "expected mirror event for stderr: {:?}",
                io_events
            );
            let stderr_payload = &stderr_mirror.unwrap().1;
            assert!(
                String::from_utf8_lossy(stderr_payload).contains("fd stderr"),
                "mirror stderr payload missing expected text"
            );

            assert!(io_events.iter().any(|(meta, payload)| {
                meta.stream == "stdout"
                    && !meta.flags.iter().any(|flag| flag == "mirror")
                    && String::from_utf8_lossy(payload).contains("proxy line")
            }));

            reset_policy(py);
        });
    }

    #[cfg(unix)]
    #[test]
    fn fd_mirror_disabled_does_not_capture_os_write() {
        Python::with_gil(|py| {
            reset_policy(py);
            policy::configure_policy_py(
                Some("abort"),
                Some(false),
                Some(false),
                None,
                None,
                Some(false),
                Some(true),
                Some(false),
            )
            .expect("enable proxies without fd fallback");

            ensure_test_module(py);
            let tmp = tempfile::tempdir().expect("tempdir");
            let script_path = tmp.path().join("fd_disabled.py");
            std::fs::write(
                &script_path,
                format!(
                    "{PRELUDE}\nimport os\nprint('proxy line')\nos.write(1, b'fd stdout\\n')\nos.write(2, b'fd stderr\\n')\n"
                ),
            )
            .expect("write script");

            let mut tracer = RuntimeTracer::new(
                script_path.to_string_lossy().as_ref(),
                &[],
                TraceEventsFileFormat::Json,
                None,
                None,
            );
            let outputs = TraceOutputPaths::new(tmp.path(), TraceEventsFileFormat::Json);
            tracer.begin(&outputs, 1).expect("begin tracer");
            tracer
                .install_io_capture(py, &policy::policy_snapshot())
                .expect("install io capture");

            {
                let _guard = ScopedTracer::new(&mut tracer);
                LAST_OUTCOME.with(|cell| cell.set(None));
                let run_code = format!(
                    "import runpy\nrunpy.run_path(r\"{}\")",
                    script_path.display()
                );
                let run_code_c = CString::new(run_code).expect("script contains nul byte");
                py.run(run_code_c.as_c_str(), None, None)
                    .expect("execute fd script");
            }

            tracer.finish(py).expect("finish tracer");

            let io_events: Vec<(IoMetadata, Vec<u8>)> = tracer
                .writer
                .events
                .iter()
                .filter_map(|event| match event {
                    TraceLowLevelEvent::Event(record) => {
                        let metadata: IoMetadata = serde_json::from_str(&record.metadata).ok()?;
                        Some((metadata, record.content.as_bytes().to_vec()))
                    }
                    _ => None,
                })
                .collect();

            assert!(
                !io_events
                    .iter()
                    .any(|(meta, _)| meta.flags.iter().any(|flag| flag == "mirror")),
                "mirror events should not be present when fallback disabled"
            );

            assert!(
                !io_events.iter().any(|(_, payload)| {
                    String::from_utf8_lossy(payload).contains("fd stdout")
                        || String::from_utf8_lossy(payload).contains("fd stderr")
                }),
                "native os.write payload unexpectedly captured without fallback"
            );

            assert!(io_events.iter().any(|(meta, payload)| {
                meta.stream == "stdout" && String::from_utf8_lossy(payload).contains("proxy line")
            }));

            reset_policy(py);
        });
    }

    #[pyfunction]
    fn capture_line(py: Python<'_>, code: Bound<'_, PyCode>, lineno: u32) -> PyResult<()> {
        ffi::wrap_pyfunction("test_capture_line", || {
            ACTIVE_TRACER.with(|cell| -> PyResult<()> {
                let ptr = cell.get();
                if ptr.is_null() {
                    panic!("No active RuntimeTracer for capture_line");
                }
                unsafe {
                    let tracer = &mut *ptr;
                    let wrapper = CodeObjectWrapper::new(py, &code);
                    match tracer.on_line(py, &wrapper, lineno) {
                        Ok(outcome) => {
                            LAST_OUTCOME.with(|cell| cell.set(Some(outcome)));
                            Ok(())
                        }
                        Err(err) => Err(err),
                    }
                }
            })?;
            Ok(())
        })
    }

    #[pyfunction]
    fn capture_return_event(
        py: Python<'_>,
        code: Bound<'_, PyCode>,
        value: Bound<'_, PyAny>,
    ) -> PyResult<()> {
        ffi::wrap_pyfunction("test_capture_return_event", || {
            ACTIVE_TRACER.with(|cell| -> PyResult<()> {
                let ptr = cell.get();
                if ptr.is_null() {
                    panic!("No active RuntimeTracer for capture_return_event");
                }
                unsafe {
                    let tracer = &mut *ptr;
                    let wrapper = CodeObjectWrapper::new(py, &code);
                    match tracer.on_py_return(py, &wrapper, 0, &value) {
                        Ok(outcome) => {
                            LAST_OUTCOME.with(|cell| cell.set(Some(outcome)));
                            Ok(())
                        }
                        Err(err) => Err(err),
                    }
                }
            })?;
            Ok(())
        })
    }

    const PRELUDE: &str = r#"
import inspect
from test_tracer import capture_line, capture_return_event

def snapshot(line=None):
    frame = inspect.currentframe().f_back
    lineno = frame.f_lineno if line is None else line
    capture_line(frame.f_code, lineno)

def snap(value):
    frame = inspect.currentframe().f_back
    capture_line(frame.f_code, frame.f_lineno)
    return value

def emit_return(value):
    frame = inspect.currentframe().f_back
    capture_return_event(frame.f_code, value)
    return value
"#;

    #[derive(Debug, Clone, PartialEq)]
    enum SimpleValue {
        None,
        Bool(bool),
        Int(i64),
        String(String),
        Tuple(Vec<SimpleValue>),
        Sequence(Vec<SimpleValue>),
        Raw(String),
    }

    impl SimpleValue {
        fn from_value(value: &ValueRecord) -> Self {
            match value {
                ValueRecord::None { .. } => SimpleValue::None,
                ValueRecord::Bool { b, .. } => SimpleValue::Bool(*b),
                ValueRecord::Int { i, .. } => SimpleValue::Int(*i),
                ValueRecord::String { text, .. } => SimpleValue::String(text.clone()),
                ValueRecord::Tuple { elements, .. } => {
                    SimpleValue::Tuple(elements.iter().map(SimpleValue::from_value).collect())
                }
                ValueRecord::Sequence { elements, .. } => {
                    SimpleValue::Sequence(elements.iter().map(SimpleValue::from_value).collect())
                }
                ValueRecord::Raw { r, .. } => SimpleValue::Raw(r.clone()),
                ValueRecord::Error { msg, .. } => SimpleValue::Raw(msg.clone()),
                other => SimpleValue::Raw(format!("{other:?}")),
            }
        }
    }

    #[derive(Debug)]
    struct Snapshot {
        line: i64,
        vars: BTreeMap<String, SimpleValue>,
    }

    fn collect_snapshots(events: &[TraceLowLevelEvent]) -> Vec<Snapshot> {
        let mut names: Vec<String> = Vec::new();
        let mut snapshots: Vec<Snapshot> = Vec::new();
        let mut current: Option<Snapshot> = None;
        for event in events {
            match event {
                TraceLowLevelEvent::VariableName(name) => names.push(name.clone()),
                TraceLowLevelEvent::Step(step) => {
                    if let Some(snapshot) = current.take() {
                        snapshots.push(snapshot);
                    }
                    current = Some(Snapshot {
                        line: step.line.0,
                        vars: BTreeMap::new(),
                    });
                }
                TraceLowLevelEvent::Value(FullValueRecord { variable_id, value }) => {
                    if let Some(snapshot) = current.as_mut() {
                        let index = variable_id.0;
                        let name = names
                            .get(index)
                            .cloned()
                            .unwrap_or_else(|| panic!("Missing variable name for id {}", index));
                        snapshot.vars.insert(name, SimpleValue::from_value(value));
                    }
                }
                _ => {}
            }
        }
        if let Some(snapshot) = current.take() {
            snapshots.push(snapshot);
        }
        snapshots
    }

    fn ensure_test_module(py: Python<'_>) {
        let module = PyModule::new(py, "test_tracer").expect("create module");
        module
            .add_function(wrap_pyfunction!(capture_line, &module).expect("wrap capture_line"))
            .expect("add function");
        module
            .add_function(
                wrap_pyfunction!(capture_return_event, &module).expect("wrap capture_return_event"),
            )
            .expect("add return capture function");
        py.import("sys")
            .expect("import sys")
            .getattr("modules")
            .expect("modules attr")
            .set_item("test_tracer", module)
            .expect("insert module");
    }

    fn run_traced_script(body: &str) -> Vec<Snapshot> {
        Python::with_gil(|py| {
            let mut tracer =
                RuntimeTracer::new("test.py", &[], TraceEventsFileFormat::Json, None, None);
            ensure_test_module(py);
            let tmp = tempfile::tempdir().expect("create temp dir");
            let script_path = tmp.path().join("script.py");
            let script = format!("{PRELUDE}\n{body}");
            std::fs::write(&script_path, &script).expect("write script");
            {
                let _guard = ScopedTracer::new(&mut tracer);
                LAST_OUTCOME.with(|cell| cell.set(None));
                let run_code = format!(
                    "import runpy\nrunpy.run_path(r\"{}\")",
                    script_path.display()
                );
                let run_code_c = CString::new(run_code).expect("script contains nul byte");
                py.run(run_code_c.as_c_str(), None, None)
                    .expect("execute test script");
            }
            collect_snapshots(&tracer.writer.events)
        })
    }

    fn write_filter(path: &Path, contents: &str) {
        fs::write(path, contents.trim_start()).expect("write filter");
    }

    #[test]
    fn trace_filter_redacts_values() {
        Python::with_gil(|py| {
            ensure_test_module(py);

            let project = tempfile::tempdir().expect("project dir");
            let project_root = project.path();
            let filters_dir = project_root.join(".codetracer");
            fs::create_dir(&filters_dir).expect("create .codetracer");
            let filter_path = filters_dir.join("filters.toml");
            write_filter(
                &filter_path,
                r#"
                [meta]
                name = "redact"
                version = 1

                [scope]
                default_exec = "trace"
                default_value_action = "allow"

                [[scope.rules]]
                selector = "pkg:app.sec"
                exec = "trace"
                value_default = "allow"

                [[scope.rules.value_patterns]]
                selector = "arg:password"
                action = "deny"

                [[scope.rules.value_patterns]]
                selector = "local:password"
                action = "deny"

                [[scope.rules.value_patterns]]
                selector = "local:secret"
                action = "deny"

                [[scope.rules.value_patterns]]
                selector = "global:shared_secret"
                action = "deny"

                [[scope.rules.value_patterns]]
                selector = "ret:literal:app.sec.sensitive"
                action = "deny"
                "#,
            );
            let config = TraceFilterConfig::from_paths(&[filter_path]).expect("load filter");
            let engine = Arc::new(TraceFilterEngine::new(config));

            let app_dir = project_root.join("app");
            fs::create_dir_all(&app_dir).expect("create app dir");
            let script_path = app_dir.join("sec.py");
            let body = r#"
shared_secret = "initial"

def sensitive(password):
    secret = "token"
    public = "visible"
    globals()['shared_secret'] = password
    snapshot()
    emit_return(password)
    return password

sensitive("s3cr3t")
"#;
            let script = format!("{PRELUDE}\n{body}", PRELUDE = PRELUDE, body = body);
            fs::write(&script_path, script).expect("write script");

            let mut tracer = RuntimeTracer::new(
                script_path.to_string_lossy().as_ref(),
                &[],
                TraceEventsFileFormat::Json,
                None,
                Some(engine),
            );

            {
                let _guard = ScopedTracer::new(&mut tracer);
                LAST_OUTCOME.with(|cell| cell.set(None));
                let run_code = format!(
                    "import runpy, sys\nsys.path.insert(0, r\"{}\")\nrunpy.run_path(r\"{}\")",
                    project_root.display(),
                    script_path.display()
                );
                let run_code_c = CString::new(run_code).expect("script contains nul byte");
                py.run(run_code_c.as_c_str(), None, None)
                    .expect("execute filtered script");
            }

            let mut variable_names: Vec<String> = Vec::new();
            for event in &tracer.writer.events {
                if let TraceLowLevelEvent::VariableName(name) = event {
                    variable_names.push(name.clone());
                }
            }

            let password_index = variable_names
                .iter()
                .position(|name| name == "password")
                .expect("password variable recorded");
            let password_value = tracer
                .writer
                .events
                .iter()
                .find_map(|event| match event {
                    TraceLowLevelEvent::Value(record) if record.variable_id.0 == password_index => {
                        Some(record.value.clone())
                    }
                    _ => None,
                })
                .expect("password value recorded");
            match password_value {
                ValueRecord::Error { ref msg, .. } => assert_eq!(msg, "<redacted>"),
                ref other => panic!("expected password argument redacted, got {other:?}"),
            }

            let snapshots = collect_snapshots(&tracer.writer.events);
            let snapshot = find_snapshot_with_vars(
                &snapshots,
                &["secret", "public", "shared_secret", "password"],
            );
            assert_var(
                snapshot,
                "secret",
                SimpleValue::Raw("<redacted>".to_string()),
            );
            assert_var(
                snapshot,
                "public",
                SimpleValue::String("visible".to_string()),
            );
            assert_var(
                snapshot,
                "shared_secret",
                SimpleValue::Raw("<redacted>".to_string()),
            );
            assert_var(
                snapshot,
                "password",
                SimpleValue::Raw("<redacted>".to_string()),
            );

            let return_record = tracer
                .writer
                .events
                .iter()
                .find_map(|event| match event {
                    TraceLowLevelEvent::Return(record) => Some(record.clone()),
                    _ => None,
                })
                .expect("return record");

            match return_record.return_value {
                ValueRecord::Error { ref msg, .. } => assert_eq!(msg, "<redacted>"),
                ref other => panic!("expected redacted return value, got {other:?}"),
            }
        });
    }

    #[test]
    fn trace_filter_metadata_includes_summary() {
        Python::with_gil(|py| {
            reset_policy(py);
            ensure_test_module(py);

            let project = tempfile::tempdir().expect("project dir");
            let project_root = project.path();
            let filters_dir = project_root.join(".codetracer");
            fs::create_dir(&filters_dir).expect("create .codetracer");
            let filter_path = filters_dir.join("filters.toml");
            write_filter(
                &filter_path,
                r#"
                [meta]
                name = "redact"
                version = 1

                [scope]
                default_exec = "trace"
                default_value_action = "allow"

                [[scope.rules]]
                selector = "pkg:app.sec"
                exec = "trace"
                value_default = "allow"

                [[scope.rules.value_patterns]]
                selector = "arg:password"
                action = "deny"

                [[scope.rules.value_patterns]]
                selector = "local:password"
                action = "deny"

                [[scope.rules.value_patterns]]
                selector = "local:secret"
                action = "deny"

                [[scope.rules.value_patterns]]
                selector = "global:shared_secret"
                action = "deny"

                [[scope.rules.value_patterns]]
                selector = "ret:literal:app.sec.sensitive"
                action = "deny"
                "#,
            );
            let config = TraceFilterConfig::from_paths(&[filter_path]).expect("load filter");
            let engine = Arc::new(TraceFilterEngine::new(config));

            let app_dir = project_root.join("app");
            fs::create_dir_all(&app_dir).expect("create app dir");
            let script_path = app_dir.join("sec.py");
            let body = r#"
shared_secret = "initial"

def sensitive(password):
    secret = "token"
    globals()['shared_secret'] = password
    snapshot()
    emit_return(password)
    return password

sensitive("s3cr3t")
"#;
            let script = format!("{PRELUDE}\n{body}", PRELUDE = PRELUDE, body = body);
            fs::write(&script_path, script).expect("write script");

            let outputs_dir = tempfile::tempdir().expect("outputs dir");
            let outputs = TraceOutputPaths::new(outputs_dir.path(), TraceEventsFileFormat::Json);

            let program = script_path.to_string_lossy().into_owned();
            let mut tracer = RuntimeTracer::new(
                &program,
                &[],
                TraceEventsFileFormat::Json,
                None,
                Some(engine),
            );
            tracer.begin(&outputs, 1).expect("begin tracer");

            {
                let _guard = ScopedTracer::new(&mut tracer);
                LAST_OUTCOME.with(|cell| cell.set(None));
                let run_code = format!(
                    "import runpy, sys\nsys.path.insert(0, r\"{}\")\nrunpy.run_path(r\"{}\")",
                    project_root.display(),
                    script_path.display()
                );
                let run_code_c = CString::new(run_code).expect("script contains nul byte");
                py.run(run_code_c.as_c_str(), None, None)
                    .expect("execute script");
            }

            tracer.finish(py).expect("finish tracer");

            let metadata_str = fs::read_to_string(outputs.metadata()).expect("read metadata");
            let metadata: serde_json::Value =
                serde_json::from_str(&metadata_str).expect("parse metadata");
            let trace_filter = metadata
                .get("trace_filter")
                .and_then(|value| value.as_object())
                .expect("trace_filter metadata");

            let filters = trace_filter
                .get("filters")
                .and_then(|value| value.as_array())
                .expect("filters array");
            assert_eq!(filters.len(), 1);
            let filter_entry = filters[0].as_object().expect("filter entry");
            assert_eq!(
                filter_entry.get("name").and_then(|v| v.as_str()),
                Some("redact")
            );

            let stats = trace_filter
                .get("stats")
                .and_then(|value| value.as_object())
                .expect("stats object");
            assert_eq!(
                stats.get("scopes_skipped").and_then(|v| v.as_u64()),
                Some(0)
            );
            let value_redactions = stats
                .get("value_redactions")
                .and_then(|value| value.as_object())
                .expect("value_redactions object");
            assert_eq!(
                value_redactions.get("argument").and_then(|v| v.as_u64()),
                Some(0)
            );
            // Argument values currently surface through local snapshots; once call-record redaction wiring lands this count should rise above zero.
            assert_eq!(
                value_redactions.get("local").and_then(|v| v.as_u64()),
                Some(2)
            );
            assert_eq!(
                value_redactions.get("global").and_then(|v| v.as_u64()),
                Some(1)
            );
            assert_eq!(
                value_redactions.get("return").and_then(|v| v.as_u64()),
                Some(1)
            );
            assert_eq!(
                value_redactions.get("attribute").and_then(|v| v.as_u64()),
                Some(0)
            );
        });
    }

    fn assert_var(snapshot: &Snapshot, name: &str, expected: SimpleValue) {
        let actual = snapshot
            .vars
            .get(name)
            .unwrap_or_else(|| panic!("{name} missing at line {}", snapshot.line));
        assert_eq!(
            actual, &expected,
            "Unexpected value for {name} at line {}",
            snapshot.line
        );
    }

    fn find_snapshot_with_vars<'a>(snapshots: &'a [Snapshot], names: &[&str]) -> &'a Snapshot {
        snapshots
            .iter()
            .find(|snap| names.iter().all(|n| snap.vars.contains_key(*n)))
            .unwrap_or_else(|| panic!("No snapshot containing variables {:?}", names))
    }

    fn assert_no_variable(snapshots: &[Snapshot], name: &str) {
        if snapshots.iter().any(|snap| snap.vars.contains_key(name)) {
            panic!("Variable {name} unexpectedly captured");
        }
    }

    #[test]
    fn captures_simple_function_locals() {
        let snapshots = run_traced_script(
            r#"
def simple_function(x):
    snapshot()
    a = 1
    snapshot()
    b = a + x
    snapshot()
    return a, b

simple_function(5)
"#,
        );

        assert_var(&snapshots[0], "x", SimpleValue::Int(5));
        assert!(!snapshots[0].vars.contains_key("a"));
        assert_var(&snapshots[1], "a", SimpleValue::Int(1));
        assert_var(&snapshots[2], "b", SimpleValue::Int(6));
    }

    #[test]
    fn captures_closure_variables() {
        let snapshots = run_traced_script(
            r#"
def outer_func(x):
    snapshot()
    y = 1
    snapshot()
    def inner_func(z):
        nonlocal y
        snapshot()
        w = x + y + z
        snapshot()
        y = w
        snapshot()
        return w
    total = inner_func(5)
    snapshot()
    return y, total

result = outer_func(2)
"#,
        );

        let inner_entry = find_snapshot_with_vars(&snapshots, &["x", "y", "z"]);
        assert_var(inner_entry, "x", SimpleValue::Int(2));
        assert_var(inner_entry, "y", SimpleValue::Int(1));

        let w_snapshot = find_snapshot_with_vars(&snapshots, &["w", "x", "y", "z"]);
        assert_var(w_snapshot, "w", SimpleValue::Int(8));

        let outer_after = find_snapshot_with_vars(&snapshots, &["total", "y"]);
        assert_var(outer_after, "total", SimpleValue::Int(8));
        assert_var(outer_after, "y", SimpleValue::Int(8));
    }

    #[test]
    fn captures_globals() {
        let snapshots = run_traced_script(
            r#"
GLOBAL_VAL = 10
counter = 0
snapshot()

def global_test():
    snapshot()
    local_copy = GLOBAL_VAL
    snapshot()
    global counter
    counter += 1
    snapshot()
    return local_copy, counter

before = counter
snapshot()
result = global_test()
snapshot()
after = counter
snapshot()
"#,
        );

        let access_global = find_snapshot_with_vars(&snapshots, &["local_copy", "GLOBAL_VAL"]);
        assert_var(access_global, "GLOBAL_VAL", SimpleValue::Int(10));
        assert_var(access_global, "local_copy", SimpleValue::Int(10));

        let last_counter = snapshots
            .iter()
            .rev()
            .find(|snap| snap.vars.contains_key("counter"))
            .expect("Expected at least one counter snapshot");
        assert_var(last_counter, "counter", SimpleValue::Int(1));
    }

    #[test]
    fn captures_class_scope() {
        let snapshots = run_traced_script(
            r#"
CONSTANT = 42
snapshot()

class MetaCounter(type):
    count = 0
    snapshot()
    def __init__(cls, name, bases, attrs):
        snapshot()
        MetaCounter.count += 1
        super().__init__(name, bases, attrs)

class Sample(metaclass=MetaCounter):
    snapshot()
    a = 10
    snapshot()
    b = a + 5
    snapshot()
    print(a, b, CONSTANT)
    snapshot()
    def method(self):
        snapshot()
        return self.a + self.b

instance = Sample()
snapshot()
instances = MetaCounter.count
snapshot()
_ = instance.method()
snapshot()
"#,
        );

        let meta_init = find_snapshot_with_vars(&snapshots, &["cls", "name", "attrs"]);
        assert_var(meta_init, "name", SimpleValue::String("Sample".to_string()));

        let class_body = find_snapshot_with_vars(&snapshots, &["a", "b"]);
        assert_var(class_body, "a", SimpleValue::Int(10));
        assert_var(class_body, "b", SimpleValue::Int(15));

        let method_snapshot = find_snapshot_with_vars(&snapshots, &["self"]);
        assert!(method_snapshot.vars.contains_key("self"));
    }

    #[test]
    fn captures_lambda_and_comprehensions() {
        let snapshots = run_traced_script(
            r#"
factor = 2
snapshot()
double = lambda y: snap(y * factor)
snapshot()
lambda_value = double(5)
snapshot()
squares = [snap(n ** 2) for n in range(3)]
snapshot()
scaled_set = {snap(n * factor) for n in range(3)}
snapshot()
mapping = {n: snap(n * factor) for n in range(3)}
snapshot()
gen_exp = (snap(n * factor) for n in range(3))
snapshot()
result_list = list(gen_exp)
snapshot()
"#,
        );

        let lambda_snapshot = find_snapshot_with_vars(&snapshots, &["y", "factor"]);
        assert_var(lambda_snapshot, "y", SimpleValue::Int(5));
        assert_var(lambda_snapshot, "factor", SimpleValue::Int(2));

        let list_comp = find_snapshot_with_vars(&snapshots, &["n", "factor"]);
        assert!(matches!(list_comp.vars.get("n"), Some(SimpleValue::Int(_))));

        let result_snapshot = find_snapshot_with_vars(&snapshots, &["result_list"]);
        assert!(matches!(
            result_snapshot.vars.get("result_list"),
            Some(SimpleValue::Sequence(_))
        ));
    }

    #[test]
    fn captures_generators_and_coroutines() {
        let snapshots = run_traced_script(
            r#"
import asyncio
snapshot()


def counter_gen(n):
    snapshot()
    total = 0
    for i in range(n):
        total += i
        snapshot()
        yield total
    snapshot()
    return total

async def async_sum(data):
    snapshot()
    total = 0
    for x in data:
        total += x
        snapshot()
        await asyncio.sleep(0)
    snapshot()
    return total

gen = counter_gen(3)
gen_results = list(gen)
snapshot()
coroutine_result = asyncio.run(async_sum([1, 2, 3]))
snapshot()
"#,
        );

        let generator_step = find_snapshot_with_vars(&snapshots, &["i", "total"]);
        assert!(matches!(
            generator_step.vars.get("i"),
            Some(SimpleValue::Int(_))
        ));

        let coroutine_steps: Vec<&Snapshot> = snapshots
            .iter()
            .filter(|snap| snap.vars.contains_key("x"))
            .collect();
        assert!(!coroutine_steps.is_empty());
        let final_coroutine_step = coroutine_steps.last().unwrap();
        assert_var(final_coroutine_step, "total", SimpleValue::Int(6));

        let coroutine_result_snapshot = find_snapshot_with_vars(&snapshots, &["coroutine_result"]);
        assert!(coroutine_result_snapshot
            .vars
            .contains_key("coroutine_result"));
    }

    #[test]
    fn captures_exception_and_with_blocks() {
        let snapshots = run_traced_script(
            r#"
import io
__file__ = "test_script.py"

def exception_and_with_demo(x):
    snapshot()
    try:
        inv = 10 / x
        snapshot()
    except ZeroDivisionError as e:
        snapshot()
        error_msg = f"Error: {e}"
        snapshot()
    else:
        snapshot()
        inv += 1
        snapshot()
    finally:
        snapshot()
        final_flag = True
        snapshot()
    with io.StringIO("dummy line") as f:
        snapshot()
        first_line = f.readline()
        snapshot()
    snapshot()
    return locals()

result1 = exception_and_with_demo(0)
snapshot()
result2 = exception_and_with_demo(5)
snapshot()
"#,
        );

        let except_snapshot = find_snapshot_with_vars(&snapshots, &["e", "error_msg"]);
        assert!(matches!(
            except_snapshot.vars.get("error_msg"),
            Some(SimpleValue::String(_))
        ));

        let finally_snapshot = find_snapshot_with_vars(&snapshots, &["final_flag"]);
        assert_var(finally_snapshot, "final_flag", SimpleValue::Bool(true));

        let with_snapshot = find_snapshot_with_vars(&snapshots, &["f", "first_line"]);
        assert!(with_snapshot.vars.contains_key("first_line"));
    }

    #[test]
    fn captures_decorators() {
        let snapshots = run_traced_script(
            r#"
setting = "Hello"
snapshot()


def my_decorator(func):
    snapshot()
    def wrapper(*args, **kwargs):
        snapshot()
        return func(*args, **kwargs)
    return wrapper

@my_decorator
def greet(name):
    snapshot()
    message = f"Hi, {name}"
    snapshot()
    return message

output = greet("World")
snapshot()
"#,
        );

        let decorator_snapshot = find_snapshot_with_vars(&snapshots, &["func", "setting"]);
        assert!(decorator_snapshot.vars.contains_key("func"));

        let wrapper_snapshot = find_snapshot_with_vars(&snapshots, &["args", "kwargs", "setting"]);
        assert!(wrapper_snapshot.vars.contains_key("args"));

        let greet_snapshot = find_snapshot_with_vars(&snapshots, &["name", "message"]);
        assert_var(
            greet_snapshot,
            "name",
            SimpleValue::String("World".to_string()),
        );
    }

    #[test]
    fn captures_dynamic_execution() {
        let snapshots = run_traced_script(
            r#"
expr_code = "dynamic_var = 99"
snapshot()
exec(expr_code)
snapshot()
check = dynamic_var + 1
snapshot()

def eval_test():
    snapshot()
    value = 10
    formula = "value * 2"
    snapshot()
    result = eval(formula)
    snapshot()
    return result

out = eval_test()
snapshot()
"#,
        );

        let exec_snapshot = find_snapshot_with_vars(&snapshots, &["dynamic_var"]);
        assert_var(exec_snapshot, "dynamic_var", SimpleValue::Int(99));

        let eval_snapshot = find_snapshot_with_vars(&snapshots, &["value", "formula"]);
        assert_var(eval_snapshot, "value", SimpleValue::Int(10));
    }

    #[test]
    fn captures_imports() {
        let snapshots = run_traced_script(
            r#"
import math
snapshot()

def import_test():
    snapshot()
    import os
    snapshot()
    constant = math.pi
    snapshot()
    cwd = os.getcwd()
    snapshot()
    return constant, cwd

val, path = import_test()
snapshot()
"#,
        );

        let global_import = find_snapshot_with_vars(&snapshots, &["math"]);
        assert!(matches!(
            global_import.vars.get("math"),
            Some(SimpleValue::Raw(_))
        ));

        let local_import = find_snapshot_with_vars(&snapshots, &["os", "constant"]);
        assert!(local_import.vars.contains_key("os"));
    }

    #[test]
    fn builtins_not_recorded() {
        let snapshots = run_traced_script(
            r#"
def builtins_test(seq):
    snapshot()
    n = len(seq)
    snapshot()
    m = max(seq)
    snapshot()
    return n, m

result = builtins_test([5, 3, 7])
snapshot()
"#,
        );

        let len_snapshot = find_snapshot_with_vars(&snapshots, &["n"]);
        assert_var(len_snapshot, "n", SimpleValue::Int(3));
        assert_no_variable(&snapshots, "len");
    }

    #[test]
    fn finish_enforces_require_trace_policy() {
        Python::with_gil(|py| {
            policy::configure_policy_py(
                Some("abort"),
                Some(true),
                Some(false),
                None,
                None,
                Some(false),
                None,
                None,
            )
            .expect("enable require_trace policy");

            let script_dir = tempfile::tempdir().expect("script dir");
            let program_path = script_dir.path().join("program.py");
            std::fs::write(&program_path, "print('hi')\n").expect("write program");

            let outputs_dir = tempfile::tempdir().expect("outputs dir");
            let outputs = TraceOutputPaths::new(outputs_dir.path(), TraceEventsFileFormat::Json);

            let mut tracer = RuntimeTracer::new(
                program_path.to_string_lossy().as_ref(),
                &[],
                TraceEventsFileFormat::Json,
                None,
                None,
            );
            tracer.begin(&outputs, 1).expect("begin tracer");

            let err = tracer
                .finish(py)
                .expect_err("finish should error when require_trace true");
            let message = err.to_string();
            assert!(
                message.contains("ERR_TRACE_MISSING"),
                "expected trace missing error, got {message}"
            );

            reset_policy(py);
        });
    }

    #[test]
    fn finish_removes_partial_outputs_when_policy_forbids_keep() {
        Python::with_gil(|py| {
            reset_policy(py);

            let script_dir = tempfile::tempdir().expect("script dir");
            let program_path = script_dir.path().join("program.py");
            std::fs::write(&program_path, "print('hi')\n").expect("write program");

            let outputs_dir = tempfile::tempdir().expect("outputs dir");
            let outputs = TraceOutputPaths::new(outputs_dir.path(), TraceEventsFileFormat::Json);

            let mut tracer = RuntimeTracer::new(
                program_path.to_string_lossy().as_ref(),
                &[],
                TraceEventsFileFormat::Json,
                None,
                None,
            );
            tracer.begin(&outputs, 1).expect("begin tracer");
            tracer.mark_failure();

            tracer.finish(py).expect("finish after failure");

            assert!(!outputs.events().exists(), "expected events file removed");
            assert!(
                !outputs.metadata().exists(),
                "expected metadata file removed"
            );
            assert!(!outputs.paths().exists(), "expected paths file removed");
        });
    }

    #[test]
    fn finish_keeps_partial_outputs_when_policy_allows() {
        Python::with_gil(|py| {
            policy::configure_policy_py(
                Some("abort"),
                Some(false),
                Some(true),
                None,
                None,
                Some(false),
                None,
                None,
            )
            .expect("enable keep_partial policy");

            let script_dir = tempfile::tempdir().expect("script dir");
            let program_path = script_dir.path().join("program.py");
            std::fs::write(&program_path, "print('hi')\n").expect("write program");

            let outputs_dir = tempfile::tempdir().expect("outputs dir");
            let outputs = TraceOutputPaths::new(outputs_dir.path(), TraceEventsFileFormat::Json);

            let mut tracer = RuntimeTracer::new(
                program_path.to_string_lossy().as_ref(),
                &[],
                TraceEventsFileFormat::Json,
                None,
                None,
            );
            tracer.begin(&outputs, 1).expect("begin tracer");
            tracer.mark_failure();

            tracer.finish(py).expect("finish after failure");

            assert!(outputs.events().exists(), "expected events file retained");
            assert!(
                outputs.metadata().exists(),
                "expected metadata file retained"
            );
            assert!(outputs.paths().exists(), "expected paths file retained");

            reset_policy(py);
        });
    }
}
