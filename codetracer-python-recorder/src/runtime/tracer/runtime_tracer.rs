use super::events::suppress_events;
use super::filtering::{FilterCoordinator, TraceDecision};
use super::io::IoCoordinator;
use super::lifecycle::LifecycleController;
use super::path_tables::PathTables;
use crate::code_object::CodeObjectWrapper;
use crate::ffi;
use crate::module_identity::{
    module_from_relative, module_name_from_packages, module_name_from_sys_path,
};
use crate::monitoring::CallbackOutcome;
use crate::policy::RecorderPolicy;
use crate::runtime::assignment_reconstructor::AssignmentReconstructor;
use crate::runtime::io_capture::{IoCaptureSettings, ScopedMuteIoCapture};
use crate::runtime::line_snapshots::LineSnapshotStore;
use crate::runtime::output_paths::TraceOutputPaths;
use crate::runtime::value_encoder::encode_value_streaming;
use crate::trace_filter::engine::TraceFilterEngine;
use codetracer_trace_types::Line;
use codetracer_trace_writer_nim::create_trace_writer;
use codetracer_trace_writer_nim::trace_writer::TraceWriter;
use codetracer_trace_writer_nim::StreamingValueEncoder;
use codetracer_trace_writer_nim::TraceEventsFileFormat;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyInt, PyString};
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::thread::ThreadId;

#[derive(Debug)]
enum ExitPayload {
    Code(i32),
    Text(Cow<'static, str>),
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ExitSummary {
    pub code: Option<i32>,
    pub label: Option<String>,
}

impl ExitPayload {
    fn is_code(&self) -> bool {
        matches!(self, ExitPayload::Code(_))
    }

    #[cfg(test)]
    fn is_text(&self, text: &str) -> bool {
        matches!(self, ExitPayload::Text(current) if current.as_ref() == text)
    }
}

#[derive(Debug)]
struct SessionExitState {
    payload: ExitPayload,
    emitted: bool,
}

impl Default for SessionExitState {
    fn default() -> Self {
        Self {
            payload: ExitPayload::Text(Cow::Borrowed("<exit>")),
            emitted: false,
        }
    }
}

impl SessionExitState {
    fn set_exit_code(&mut self, exit_code: Option<i32>) {
        if self.can_override_with_code() {
            self.payload = exit_code
                .map(ExitPayload::Code)
                .unwrap_or_else(|| ExitPayload::Text(Cow::Borrowed("<exit>")));
        }
    }

    fn mark_disabled(&mut self) {
        if !self.payload.is_code() {
            self.payload = ExitPayload::Text(Cow::Borrowed("<disabled>"));
        }
    }

    #[cfg(test)]
    fn mark_failure(&mut self) {
        if !self.payload.is_code() && !self.payload.is_text("<disabled>") {
            self.payload = ExitPayload::Text(Cow::Borrowed("<failure>"));
        }
    }

    fn can_override_with_code(&self) -> bool {
        matches!(&self.payload, ExitPayload::Text(current) if current.as_ref() == "<exit>")
    }

    fn as_bound<'py>(&self, py: Python<'py>) -> Bound<'py, PyAny> {
        match &self.payload {
            ExitPayload::Code(value) => PyInt::new(py, *value).into_any(),
            ExitPayload::Text(text) => PyString::new(py, text.as_ref()).into_any(),
        }
    }

    fn mark_emitted(&mut self) {
        self.emitted = true;
    }

    fn is_emitted(&self) -> bool {
        self.emitted
    }

    fn summary(&self) -> ExitSummary {
        match &self.payload {
            ExitPayload::Code(value) => ExitSummary {
                code: Some(*value),
                label: None,
            },
            ExitPayload::Text(text) => ExitSummary {
                code: None,
                label: Some(text.as_ref().to_string()),
            },
        }
    }
}

/// Minimal runtime tracer that maps Python sys.monitoring events to
/// runtime_tracing writer operations.
pub struct RuntimeTracer {
    pub(super) writer: Box<dyn TraceWriter + Send>,
    pub(super) format: TraceEventsFileFormat,
    pub(super) lifecycle: LifecycleController,
    pub(super) function_ids: HashMap<usize, codetracer_trace_types::FunctionId>,
    pub(super) io: IoCoordinator,
    pub(super) filter: FilterCoordinator,
    pub(super) module_name_from_globals: bool,
    /// Streaming value encoder (M58). Encodes Python values directly to CBOR
    /// bytes without building intermediate `ValueRecord` trees. Reused across
    /// steps to avoid per-value allocation overhead.
    pub(super) streaming_encoder: StreamingValueEncoder,
    /// M15: Assignment reconstructor. Caches per-code-object bytecode tables
    /// so we can reconstruct Assignment / BindVariable events on every
    /// `on_line` callback without re-disassembling the function.
    pub(super) assignment_reconstructor: AssignmentReconstructor,
    /// M15: per-frame set of variable names already bound in scope. Used to
    /// decide whether the next assignment should be preceded by a
    /// `BindVariable` event.
    pub(super) frame_bound_names: HashMap<u64, std::collections::HashSet<String>>,
    /// M15: per-frame "last line we observed an on_line for". The
    /// reconstructor emits Assignment events for the *previous* line on
    /// every fresh `on_line` callback, because in Python 3.12 the LINE
    /// event fires before the line executes — so when on_line(N) fires,
    /// locals reflect the post-state of line N-1.
    pub(super) last_line_per_frame: HashMap<u64, u32>,
    /// P1.2: per-frame "last column we surfaced via `register_step` /
    /// `write_delta_column`".  Mirrors `last_line_per_frame` so the
    /// `on_line` event handler can decide between emitting a column-only
    /// `DeltaColumn` event (same line, different column — the hot path on
    /// minified one-liner sources) and a `register_step` (line move,
    /// which resets the writer's column cursor to 1 per the canonical
    /// CTFS spec).  Reset to `None` for any frame whose cursor isn't
    /// known yet — the next `on_line` will land an absolute step.
    pub(super) last_column_per_frame: HashMap<u64, i64>,
    /// P1.1 / P1.2: whether this tracer is allowed to emit column-only
    /// `DeltaColumn` events.  Mirrors the writer's column-aware-mode
    /// flag — only the canonical CTFS multi-stream backend supports
    /// it; on the legacy `BinaryV0` format this stays `false`
    /// and the recorder falls back to `register_step`-only.
    pub(super) column_aware: bool,
    /// The `paths.dat` Layout A table of every file this trace mentions.
    pub(super) path_tables: PathTables,
    /// M15: monotonic counter mirroring the writer's call-record index so we
    /// can stamp `RValue::FunctionReturn { call_key }` with the
    /// most-recently-popped call.
    pub(super) last_call_key: i64,
    session_exit: SessionExitState,
}

impl RuntimeTracer {
    pub fn new(
        program: &str,
        args: &[String],
        format: TraceEventsFileFormat,
        activation_path: Option<&Path>,
        trace_filter: Option<Arc<TraceFilterEngine>>,
        module_name_from_globals: bool,
    ) -> Self {
        let writer = create_trace_writer(program, args, format);
        let lifecycle = LifecycleController::new(program, activation_path);
        // P1.1: column-aware mode is gated on the canonical CTFS
        // multi-stream backend.  On legacy formats the trait-default
        // `enable_column_aware_steps` is a no-op and `write_delta_column`
        // would set the writer's error string without affecting the
        // trace, so we keep the recorder's `column_aware` flag in lock-
        // step with the writer's capability.
        let column_aware = matches!(format, TraceEventsFileFormat::Ctfs);
        Self {
            writer,
            format,
            lifecycle,
            function_ids: HashMap::new(),
            io: IoCoordinator::new(),
            filter: FilterCoordinator::new(trace_filter),
            module_name_from_globals,
            streaming_encoder: StreamingValueEncoder::new(),
            assignment_reconstructor: AssignmentReconstructor::new(),
            frame_bound_names: HashMap::new(),
            last_line_per_frame: HashMap::new(),
            last_column_per_frame: HashMap::new(),
            column_aware,
            path_tables: PathTables::new(column_aware),
            last_call_key: -1,
            session_exit: SessionExitState::default(),
        }
    }

    /// Share the snapshot store with collaborators (IO capture, tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn line_snapshot_store(&self) -> Arc<LineSnapshotStore> {
        self.io.snapshot_store()
    }

    pub fn install_io_capture(&mut self, py: Python<'_>, policy: &RecorderPolicy) -> PyResult<()> {
        let settings = IoCaptureSettings {
            line_proxies: policy.io_capture.line_proxies,
            fd_mirror: policy.io_capture.fd_fallback,
        };
        self.io.install(py, settings)
    }

    pub(super) fn flush_io_before_step(&mut self, thread_id: ThreadId) {
        if self
            .io
            .flush_before_step(thread_id, &mut *self.writer, &mut self.path_tables)
        {
            self.mark_event();
        }
    }

    pub(super) fn flush_pending_io(&mut self) {
        if self.io.flush_all(&mut *self.writer, &mut self.path_tables) {
            self.mark_event();
        }
    }

    pub(super) fn emit_session_exit(&mut self, py: Python<'_>) {
        if self.session_exit.is_emitted() {
            return;
        }

        self.flush_pending_io();
        let value = self.session_exit.as_bound(py);
        let cbor =
            encode_value_streaming(py, &mut *self.writer, &mut self.streaming_encoder, &value);
        TraceWriter::register_return_cbor(&mut *self.writer, &cbor);
        self.session_exit.mark_emitted();
    }

    /// Configure output files and write initial metadata records.
    pub fn begin(&mut self, outputs: &TraceOutputPaths, start_line: u32) -> PyResult<()> {
        self.lifecycle
            .begin(
                &mut *self.writer,
                outputs,
                start_line,
                &self.filter,
                &mut self.path_tables,
            )
            .map_err(ffi::map_recorder_error)?;
        Ok(())
    }

    pub(super) fn mark_event(&mut self) {
        if suppress_events() {
            let _mute = ScopedMuteIoCapture::new();
            log::debug!("[RuntimeTracer] skipping event mark due to test injection");
            return;
        }
        self.lifecycle.mark_event();
    }

    #[cfg(test)]
    pub(super) fn mark_failure(&mut self) {
        self.session_exit.mark_failure();
        self.lifecycle.mark_failure();
    }

    pub(super) fn mark_disabled(&mut self) {
        self.session_exit.mark_disabled();
        self.lifecycle.mark_failure();
    }

    pub(super) fn record_exit_status(&mut self, exit_code: Option<i32>) {
        self.session_exit.set_exit_code(exit_code);
    }

    pub(super) fn exit_summary(&self) -> ExitSummary {
        self.session_exit.summary()
    }

    pub(super) fn evaluate_gate(
        &mut self,
        py: Python<'_>,
        code: &CodeObjectWrapper,
        allow_disable: bool,
    ) -> Option<CallbackOutcome> {
        let is_active = self
            .lifecycle
            .activation_mut()
            .should_process_event(py, code);
        if matches!(
            self.should_trace_code(py, code),
            TraceDecision::SkipAndDisable
        ) {
            return Some(if allow_disable {
                CallbackOutcome::DisableLocation
            } else {
                CallbackOutcome::Continue
            });
        }
        if !is_active {
            return Some(CallbackOutcome::Continue);
        }
        None
    }

    pub(super) fn ensure_function_id(
        &mut self,
        py: Python<'_>,
        code: &CodeObjectWrapper,
    ) -> PyResult<codetracer_trace_types::FunctionId> {
        if let Some(fid) = self.function_ids.get(&code.id()) {
            return Ok(*fid);
        }
        let name = self.function_name(py, code)?;
        let filename = code.filename(py)?;
        let first_line = code.first_line(py)?;
        // A function registration interns its file; give the file its
        // per-line table first.
        self.path_tables
            .register(&mut *self.writer, Path::new(filename));
        let function_id = TraceWriter::ensure_function_id(
            &mut *self.writer,
            name.as_str(),
            Path::new(filename),
            Line(first_line as i64),
        );
        self.function_ids.insert(code.id(), function_id);
        Ok(function_id)
    }

    pub(super) fn should_trace_code(
        &mut self,
        py: Python<'_>,
        code: &CodeObjectWrapper,
    ) -> TraceDecision {
        self.filter.decide(py, code)
    }

    fn function_name(&self, py: Python<'_>, code: &CodeObjectWrapper) -> PyResult<String> {
        let qualname = code.qualname(py)?;
        if qualname == "<module>" {
            Ok(self
                .derive_module_name(py, code)
                .map(|module| format!("<{module}>"))
                .unwrap_or_else(|| qualname.to_string()))
        } else {
            Ok(qualname.to_string())
        }
    }

    fn derive_module_name(&self, py: Python<'_>, code: &CodeObjectWrapper) -> Option<String> {
        if self.module_name_from_globals {
            if let Some(name) = self.filter.module_name_hint(code.id()) {
                return Some(name);
            }
        }

        let resolution = self.filter.cached_resolution(py, code);
        if let Some(resolution) = resolution.as_ref() {
            if let Some(name) = resolution.module_name() {
                return Some(name.to_string());
            }
            if let Some(relative) = resolution.relative_path() {
                if let Some(name) = module_from_relative(relative) {
                    return Some(name);
                }
            }
            if let Some(absolute) = resolution.absolute_path() {
                if let Some(name) = module_name_from_sys_path(py, Path::new(absolute)) {
                    return Some(name);
                }
                if let Some(name) = module_name_from_packages(Path::new(absolute)) {
                    return Some(name);
                }
            }
        }

        if let Ok(filename) = code.filename(py) {
            let path = Path::new(filename);
            if let Some(name) = module_name_from_sys_path(py, path) {
                return Some(name);
            }
            if let Some(name) = module_name_from_packages(path) {
                return Some(name);
            }
        }

        None
    }
}

#[cfg(test)]
impl RuntimeTracer {
    fn function_name_for_test(&self, py: Python<'_>, code: &CodeObjectWrapper) -> PyResult<String> {
        self.function_name(py, code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitoring::{CallbackOutcome, Tracer};
    use crate::policy;
    use crate::runtime::tracer::filtering::is_real_filename;
    use crate::trace_filter::config::TraceFilterConfig;
    use codetracer_trace_types::{FullValueRecord, StepRecord, TraceLowLevelEvent, ValueRecord};
    use pyo3::types::{PyAny, PyCode, PyModule};
    use pyo3::wrap_pyfunction;
    use serde::Deserialize;
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use std::ffi::CString;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::thread;

    thread_local! {
        static ACTIVE_TRACER: Cell<*mut RuntimeTracer> = Cell::new(std::ptr::null_mut());
        static LAST_OUTCOME: Cell<Option<CallbackOutcome>> = Cell::new(None);
    }

    const BUILTIN_TRACE_FILTER: &str =
        include_str!("../../../resources/trace_filters/builtin_default.toml");

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
            Some(false),
            Some(false),
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
            let mut tracer = RuntimeTracer::new(
                "test.py",
                &[],
                TraceEventsFileFormat::BinaryV0,
                None,
                None,
                false,
            );
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
                tracer.writer.events().is_empty(),
                "expected no events for synthetic filename"
            );
            let outcome = last_outcome();
            assert!(
                matches!(
                    outcome,
                    Some(CallbackOutcome::DisableLocation | CallbackOutcome::Continue)
                ),
                "expected DisableLocation or Continue (when CPython refuses to disable an event), got {:?}",
                outcome
            );

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
                TraceDecision::SkipAndDisable
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
                TraceEventsFileFormat::BinaryV0,
                Some(script_path.as_path()),
                None,
                false,
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
                .events()
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
            assert!(!tracer.lifecycle.activation().is_active());
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
                TraceEventsFileFormat::BinaryV0,
                None,
                None,
                false,
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
                .events()
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
                Some(false),
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
                TraceEventsFileFormat::BinaryV0,
                None,
                None,
                false,
            );
            let outputs = TraceOutputPaths::new(tmp.path(), TraceEventsFileFormat::BinaryV0, "program.py");
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
                .events()
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
                Some(false),
                Some(false),
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
                TraceEventsFileFormat::BinaryV0,
                None,
                None,
                false,
            );
            let outputs = TraceOutputPaths::new(tmp.path(), TraceEventsFileFormat::BinaryV0, "program.py");
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
                .events()
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
            let stdout_payload = &stdout_mirror.expect("stdout mirror event present").1;
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
            let stderr_payload = &stderr_mirror.expect("stderr mirror event present").1;
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
                Some(false),
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
                TraceEventsFileFormat::BinaryV0,
                None,
                None,
                false,
            );
            let outputs = TraceOutputPaths::new(tmp.path(), TraceEventsFileFormat::BinaryV0, "program.py");
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
                .events()
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
    fn capture_py_start(py: Python<'_>, code: Bound<'_, PyCode>, offset: i32) -> PyResult<()> {
        ffi::wrap_pyfunction("test_capture_py_start", || {
            ACTIVE_TRACER.with(|cell| -> PyResult<()> {
                let ptr = cell.get();
                if ptr.is_null() {
                    panic!("No active RuntimeTracer for capture_py_start");
                }
                unsafe {
                    let tracer = &mut *ptr;
                    let wrapper = CodeObjectWrapper::new(py, &code);
                    match tracer.on_py_start(py, &wrapper, offset) {
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
from test_tracer import capture_line, capture_return_event, capture_py_start

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

def start_call():
    frame = inspect.currentframe().f_back
    capture_py_start(frame.f_code, frame.f_lasti)
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
            .add_function(
                wrap_pyfunction!(capture_py_start, &module).expect("wrap capture_py_start"),
            )
            .expect("add py_start capture function");
        module
            .add_function(wrap_pyfunction!(capture_line, &module).expect("wrap capture_line"))
            .expect("add line capture function");
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
            let mut tracer = RuntimeTracer::new(
                "test.py",
                &[],
                TraceEventsFileFormat::BinaryV0,
                None,
                None,
                false,
            );
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
            collect_snapshots(tracer.writer.events())
        })
    }

    fn write_filter(path: &Path, contents: &str) {
        fs::write(path, contents.trim_start()).expect("write filter");
    }

    fn install_drop_everything_filter(project_root: &Path) -> PathBuf {
        let filters_dir = project_root.join(".codetracer");
        fs::create_dir(&filters_dir).expect("create .codetracer");
        let drop_filter_path = filters_dir.join("drop-filter.toml");
        write_filter(
            &drop_filter_path,
            r#"
            [meta]
            name = "drop-all"
            version = 1

            [scope]
            default_exec = "trace"
            default_value_action = "drop"
            "#,
        );
        drop_filter_path
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
                action = "redact"

                [[scope.rules.value_patterns]]
                selector = "local:password"
                action = "redact"

                [[scope.rules.value_patterns]]
                selector = "local:secret"
                action = "redact"

                [[scope.rules.value_patterns]]
                selector = "global:shared_secret"
                action = "redact"

                [[scope.rules.value_patterns]]
                selector = "ret:literal:app.sec.sensitive"
                action = "redact"

                [[scope.rules.value_patterns]]
                selector = "local:internal"
                action = "drop"
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
    internal = "hidden"
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
                TraceEventsFileFormat::BinaryV0,
                None,
                Some(engine),
                false,
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
            for event in tracer.writer.events() {
                if let TraceLowLevelEvent::VariableName(name) = event {
                    variable_names.push(name.clone());
                }
            }
            assert!(
                !variable_names.iter().any(|name| name == "internal"),
                "internal variable should not be recorded"
            );

            let password_index = variable_names
                .iter()
                .position(|name| name == "password")
                .expect("password variable recorded");
            let password_value = tracer
                .writer
                .events()
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

            let snapshots = collect_snapshots(tracer.writer.events());
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
            assert_no_variable(&snapshots, "internal");

            let return_record = tracer
                .writer
                .events()
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
    fn module_import_records_module_name() {
        Python::with_gil(|py| {
            let project = tempfile::tempdir().expect("project dir");
            let pkg_root = project.path().join("lib");
            let pkg_dir = pkg_root.join("my_pkg");
            fs::create_dir_all(&pkg_dir).expect("create package dir");
            let module_path = pkg_dir.join("mod.py");
            fs::write(&module_path, "value = 1\n").expect("write module file");

            let sys = py.import("sys").expect("import sys");
            let sys_path = sys.getattr("path").expect("sys.path");
            sys_path
                .call_method1("insert", (0, pkg_root.to_string_lossy().as_ref()))
                .expect("insert temp root");

            let tracer = RuntimeTracer::new(
                "runner.py",
                &[],
                TraceEventsFileFormat::BinaryV0,
                None,
                None,
                false,
            );

            let builtins = py.import("builtins").expect("builtins");
            let compile = builtins.getattr("compile").expect("compile builtin");
            let code_obj: Bound<'_, PyCode> = compile
                .call1((
                    "value = 1\n",
                    module_path.to_string_lossy().as_ref(),
                    "exec",
                ))
                .expect("compile module code")
                .downcast_into()
                .expect("PyCode");

            let wrapper = CodeObjectWrapper::new(py, &code_obj);
            let resolved = tracer
                .function_name_for_test(py, &wrapper)
                .expect("derive function name");

            assert_eq!(resolved, "<my_pkg.mod>");

            sys_path.call_method1("pop", (0,)).expect("pop temp root");
        });
    }

    #[test]
    fn user_drop_default_overrides_builtin_allowance() {
        Python::with_gil(|py| {
            ensure_test_module(py);

            let project = tempfile::tempdir().expect("project dir");
            let project_root = project.path();
            let drop_filter_path = install_drop_everything_filter(project_root);

            let config = TraceFilterConfig::from_inline_and_paths(
                &[("builtin-default", BUILTIN_TRACE_FILTER)],
                &[drop_filter_path.clone()],
            )
            .expect("load filter chain");
            let engine = Arc::new(TraceFilterEngine::new(config));

            let app_dir = project_root.join("app");
            fs::create_dir_all(&app_dir).expect("create app dir");
            let script_path = app_dir.join("dropper.py");
            let body = r#"
def dropper():
    secret = "token"
    public = 42
    snapshot()
    emit_return(secret)
    return secret

dropper()
"#;
            let script = format!("{PRELUDE}\n{body}", PRELUDE = PRELUDE, body = body);
            fs::write(&script_path, script).expect("write script");

            let mut tracer = RuntimeTracer::new(
                script_path.to_string_lossy().as_ref(),
                &[],
                TraceEventsFileFormat::BinaryV0,
                None,
                Some(engine),
                false,
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
                    .expect("execute dropper script");
            }

            let mut variable_names: Vec<String> = Vec::new();
            let mut return_values: Vec<ValueRecord> = Vec::new();
            for event in tracer.writer.events() {
                match event {
                    TraceLowLevelEvent::VariableName(name) => variable_names.push(name.clone()),
                    TraceLowLevelEvent::Return(record) => {
                        return_values.push(record.return_value.clone())
                    }
                    _ => {}
                }
            }
            assert!(
                variable_names.is_empty(),
                "expected no variables captured, found {:?}",
                variable_names
            );
            assert_eq!(
                return_values.len(),
                1,
                "return event should remain balanced"
            );
            match &return_values[0] {
                ValueRecord::Error { msg, .. } => assert_eq!(msg, "<dropped>"),
                other => panic!("expected dropped sentinel return value, got {other:?}"),
            }
        });
    }

    #[test]
    fn drop_filters_keep_call_return_pairs_balanced() {
        Python::with_gil(|py| {
            ensure_test_module(py);

            let project = tempfile::tempdir().expect("project dir");
            let project_root = project.path();
            let drop_filter_path = install_drop_everything_filter(project_root);

            let config = TraceFilterConfig::from_inline_and_paths(
                &[("builtin-default", BUILTIN_TRACE_FILTER)],
                &[drop_filter_path.clone()],
            )
            .expect("load filter chain");
            let engine = Arc::new(TraceFilterEngine::new(config));

            let app_dir = project_root.join("app");
            fs::create_dir_all(&app_dir).expect("create app dir");
            let script_path = app_dir.join("classes.py");
            let body = r#"
def initializer(label):
    start_call()
    return emit_return(label.upper())

class Alpha:
    TOKEN = initializer("alpha")

class Beta:
    TOKEN = initializer("beta")

class Gamma:
    TOKEN = initializer("gamma")

initializer("omega")
"#;
            let script = format!("{PRELUDE}\n{body}", PRELUDE = PRELUDE, body = body);
            fs::write(&script_path, script).expect("write script");

            let mut tracer = RuntimeTracer::new(
                script_path.to_string_lossy().as_ref(),
                &[],
                TraceEventsFileFormat::BinaryV0,
                None,
                Some(engine),
                false,
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
                    .expect("execute classes script");
            }

            let mut call_count = 0usize;
            let mut return_count = 0usize;
            for event in tracer.writer.events() {
                match event {
                    TraceLowLevelEvent::Call(_) => call_count += 1,
                    TraceLowLevelEvent::Return(_) => return_count += 1,
                    _ => {}
                }
            }
            assert!(
                call_count >= 4,
                "expected at least four call events, saw {call_count}"
            );
            assert_eq!(
                call_count, return_count,
                "drop filters must keep call/return pairs balanced"
            );
        });
    }

    #[test]
    fn finish_emits_toplevel_return_with_exit_code() {
        Python::with_gil(|py| {
            reset_policy(py);

            let script_dir = tempfile::tempdir().expect("script dir");
            let program_path = script_dir.path().join("program.py");
            std::fs::write(&program_path, "print('hi')\n").expect("write program");

            let outputs_dir = tempfile::tempdir().expect("outputs dir");
            let outputs =
                TraceOutputPaths::new(outputs_dir.path(), TraceEventsFileFormat::BinaryV0, "program.py");

            let mut tracer = RuntimeTracer::new(
                program_path.to_string_lossy().as_ref(),
                &[],
                TraceEventsFileFormat::BinaryV0,
                None,
                None,
                false,
            );
            tracer.begin(&outputs, 1).expect("begin tracer");
            tracer.record_exit_status(Some(7));

            tracer.finish(py).expect("finish tracer");

            let mut exit_value: Option<ValueRecord> = None;
            for event in tracer.writer.events() {
                if let TraceLowLevelEvent::Return(record) = event {
                    exit_value = Some(record.return_value.clone());
                }
            }

            let exit_value = exit_value.expect("expected toplevel return value");
            match exit_value {
                ValueRecord::Int { i, .. } => assert_eq!(i, 7),
                other => panic!("expected integer exit value, got {other:?}"),
            }
        });
    }

    /// The trace-filter provenance block of a `meta.dat` buffer
    /// (`internal-files.md` §"Flag bit 3 -- Trace filter provenance"):
    /// `None` when flag bit 3 is clear, else the `(path, sha256 hex)`
    /// entries in composition order.
    fn decode_filter_provenance(meta_dat: &[u8]) -> Option<Vec<(String, String)>> {
        let meta = codetracer_trace_writer::meta_dat::decode_meta_dat(meta_dat)
            .unwrap_or_else(|err| panic!("meta.dat does not decode: {err}"));
        assert_eq!(
            meta.flags & 0x07,
            0,
            "unexpected meta.dat blocks before provenance"
        );
        assert!(
            meta.trailing.is_empty(),
            "meta.dat carries {} bytes after its blocks",
            meta.trailing.len()
        );
        meta.blocks.filter_provenance.map(|entries| {
            entries
                .into_iter()
                .map(|entry| {
                    let sha: String = entry.sha256.iter().map(|b| format!("{b:02x}")).collect();
                    (entry.path, sha)
                })
                .collect()
        })
    }

    #[test]
    fn trace_filter_provenance_is_recorded_in_meta_dat() {
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
                selector = "local:password"
                action = "redact"
                "#,
            );
            let config =
                TraceFilterConfig::from_paths(&[filter_path.clone()]).expect("load filter");
            let engine = Arc::new(TraceFilterEngine::new(config));
            let expected: Vec<(String, String)> = engine
                .summary()
                .entries
                .iter()
                .map(|entry| {
                    (
                        entry.path.to_string_lossy().into_owned(),
                        entry.sha256.clone(),
                    )
                })
                .collect();
            assert!(
                expected
                    .iter()
                    .any(|(path, _)| Path::new(path) == filter_path),
                "the filter chain should name {}: {expected:?}",
                filter_path.display()
            );

            let app_dir = project_root.join("app");
            fs::create_dir_all(&app_dir).expect("create app dir");
            let script_path = app_dir.join("sec.py");
            let body = r#"
def sensitive(password):
    secret = "token"
    snapshot()
    return password

sensitive("s3cr3t")
"#;
            let script = format!("{PRELUDE}\n{body}", PRELUDE = PRELUDE, body = body);
            fs::write(&script_path, script).expect("write script");

            let outputs_dir = tempfile::tempdir().expect("outputs dir");
            let program = script_path.to_string_lossy().into_owned();
            let outputs = TraceOutputPaths::new(outputs_dir.path(), TraceEventsFileFormat::Ctfs, &program);

            let mut tracer = RuntimeTracer::new(
                &program,
                &[],
                TraceEventsFileFormat::Ctfs,
                None,
                Some(engine),
                false,
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

            // The Nim writer names the container after the recorded program.
            let containers: Vec<_> = fs::read_dir(outputs_dir.path())
                .expect("list outputs dir")
                .map(|entry| entry.expect("dir entry").path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "ct"))
                .collect();
            assert_eq!(
                containers.len(),
                1,
                "expected one .ct container: {containers:?}"
            );
            let mut reader = codetracer_ctfs::CtfsReader::open(&containers[0])
                .unwrap_or_else(|err| panic!("open {}: {err:?}", containers[0].display()));
            let meta_dat = reader.read_file("meta.dat").expect("read meta.dat");
            let recorded = decode_filter_provenance(&meta_dat)
                .expect("meta.dat flag bit 3 (trace-filter provenance) is clear");
            assert_eq!(recorded, expected, "meta.dat filter provenance");
        });
    }

    /// The per-line byte lengths of `source`: one entry per line, the line's
    /// bytes without its newline.
    fn source_line_lengths(source: &str) -> Vec<u32> {
        let mut lines: Vec<u32> = source.split('\n').map(|line| line.len() as u32).collect();
        if source.ends_with('\n') {
            lines.pop();
        }
        lines
    }

    /// `trace-events.md` §"Per-File Contiguous Integer Ranges": in a
    /// column-aware trace every `paths.dat` record carries its file's
    /// per-line table, and `file_size` (the table's sum) is never zero.
    /// The table must be the source's, for files first reached by a call
    /// or an import as much as for the activation script.
    #[test]
    fn every_column_aware_path_record_carries_its_source_line_table() {
        Python::with_gil(|py| {
            reset_policy(py);
            ensure_test_module(py);

            let project = tempfile::tempdir().expect("project dir");
            let project_root = project.path();
            let helper_path = project_root.join("ct_path_table_helper.py");
            // The helper reports its own call and line events, so it is first
            // reached through a call record, as an imported module is.
            let helper_source = "import inspect\nfrom test_tracer import capture_line, capture_py_start\n\n\ndef double(value):\n    frame = inspect.currentframe()\n    capture_py_start(frame.f_code, frame.f_lasti)\n    doubled = value * 2\n    capture_line(frame.f_code, frame.f_lineno)\n    return doubled\n";
            fs::write(&helper_path, helper_source).expect("write helper");

            let script_path = project_root.join("main.py");
            let body = r#"
import ct_path_table_helper

result = ct_path_table_helper.double(21)
snapshot()
"#;
            let script = format!("{PRELUDE}\n{body}", PRELUDE = PRELUDE, body = body);
            fs::write(&script_path, &script).expect("write script");

            let outputs_dir = tempfile::tempdir().expect("outputs dir");
            let program = script_path.to_string_lossy().into_owned();
            let outputs = TraceOutputPaths::new(outputs_dir.path(), TraceEventsFileFormat::Ctfs, &program);
            let mut tracer = RuntimeTracer::new(
                &program,
                &[],
                TraceEventsFileFormat::Ctfs,
                None,
                None,
                false,
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

            let containers: Vec<_> = fs::read_dir(outputs_dir.path())
                .expect("list outputs dir")
                .map(|entry| entry.expect("dir entry").path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "ct"))
                .collect();
            assert_eq!(
                containers.len(),
                1,
                "expected one .ct container: {containers:?}"
            );
            let mut reader = codetracer_ctfs::CtfsReader::open(&containers[0])
                .unwrap_or_else(|err| panic!("open {}: {err:?}", containers[0].display()));
            let tables =
                codetracer_trace_reader::interning_tables_reader::InterningTablesReader::open(
                    &mut reader,
                )
                .expect("read interning tables")
                .expect("the trace has interning tables");
            assert!(
                tables.is_column_aware(),
                "the CTFS trace should be column-aware"
            );

            let expected: std::collections::HashMap<String, Vec<u32>> = [
                (
                    script_path.to_string_lossy().into_owned(),
                    source_line_lengths(&script),
                ),
                (
                    helper_path.to_string_lossy().into_owned(),
                    source_line_lengths(helper_source),
                ),
            ]
            .into_iter()
            .collect();
            let mut seen = Vec::new();
            for id in 0..tables.path_count() as u64 {
                let path = tables.path_str(id).expect("path");
                let line_lengths = tables.path_line_lengths(id).expect("line lengths");
                assert!(
                    line_lengths.iter().any(|&len| len > 0),
                    "paths.dat record {id} ({path}) has file_size 0: line table {line_lengths:?}"
                );
                if let Some(want) = expected.get(&path) {
                    assert_eq!(
                        &line_lengths, want,
                        "paths.dat record {id} ({path}) line table"
                    );
                    seen.push(path);
                }
            }
            for path in expected.keys() {
                assert!(
                    seen.contains(path),
                    "{path} has no paths.dat record; recorded {seen:?}"
                );
            }
        });
    }

    /// The single `.ct` container a recording wrote into `dir`.
    fn recorded_container(dir: &Path) -> std::path::PathBuf {
        let containers: Vec<_> = fs::read_dir(dir)
            .expect("list outputs dir")
            .map(|entry| entry.expect("dir entry").path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "ct"))
            .collect();
        assert_eq!(
            containers.len(),
            1,
            "expected one .ct container: {containers:?}"
        );
        containers[0].clone()
    }

    /// `internal-files.md` §"`paths.dat` Layout A": a file whose source
    /// cannot be read is registered with the conventional table, 100000
    /// lines of 1024 positions, and a step on it whose column exceeds 1024
    /// is recorded at column 1024.
    #[test]
    fn an_unreadable_source_gets_the_conventional_table_and_clamped_columns() {
        Python::with_gil(|py| {
            reset_policy(py);
            ensure_test_module(py);

            let project = tempfile::tempdir().expect("project dir");
            let project_root = project.path();
            // Compiled under a path that does not exist on disk, as code
            // whose file was deleted after import is.
            let gone_path = project_root.join("gone.py");
            let padding = vec!["1"; 600].join(" + ");
            let far_line = format!(
                "    ({padding}); far_store = 5; capture_line(frame.f_code, frame.f_lineno)"
            );
            let gone_source = format!(
                "import inspect\nfrom test_tracer import capture_line, capture_py_start\n\n\ndef far():\n    frame = inspect.currentframe()\n    capture_py_start(frame.f_code, frame.f_lasti)\n{far_line}\n    return far_store\n"
            );
            let far_lineno = 8u32;
            let store_column = far_line.find("far_store").expect("store") as u32 + 1;
            assert!(
                store_column > 1024,
                "the store must sit past column 1024: {store_column}"
            );

            let script_path = project_root.join("main.py");
            let body = format!(
                "\nnamespace = {{}}\nexec(compile({gone_source:?}, {gone:?}, \"exec\"), namespace)\nnamespace[\"far\"]()\nsnapshot()\n",
                gone = gone_path.to_string_lossy()
            );
            let script = format!("{PRELUDE}\n{body}", PRELUDE = PRELUDE, body = body);
            fs::write(&script_path, &script).expect("write script");

            let outputs_dir = tempfile::tempdir().expect("outputs dir");
            let program = script_path.to_string_lossy().into_owned();
            let outputs = TraceOutputPaths::new(outputs_dir.path(), TraceEventsFileFormat::Ctfs, &program);
            let mut tracer = RuntimeTracer::new(
                &program,
                &[],
                TraceEventsFileFormat::Ctfs,
                None,
                None,
                false,
            );
            tracer.begin(&outputs, 1).expect("begin tracer");
            {
                let _guard = ScopedTracer::new(&mut tracer);
                LAST_OUTCOME.with(|cell| cell.set(None));
                let run_code = format!(
                    "import runpy\nrunpy.run_path(r\"{}\")",
                    script_path.display()
                );
                let run_code_c = CString::new(run_code).expect("script contains nul byte");
                py.run(run_code_c.as_c_str(), None, None)
                    .expect("execute script");
            }
            tracer.finish(py).expect("finish tracer");

            let container = recorded_container(outputs_dir.path());
            let mut reader = codetracer_ctfs::CtfsReader::open(&container)
                .unwrap_or_else(|err| panic!("open {}: {err:?}", container.display()));
            let tables =
                codetracer_trace_reader::interning_tables_reader::InterningTablesReader::open(
                    &mut reader,
                )
                .expect("read interning tables")
                .expect("the trace has interning tables");
            let mut all_tables = Vec::new();
            let mut gone_id = None;
            for id in 0..tables.path_count() as u64 {
                let path = tables.path_str(id).expect("path");
                let line_lengths = tables.path_line_lengths(id).expect("line lengths");
                if Path::new(&path) == gone_path {
                    gone_id = Some(id);
                    assert!(
                        line_lengths.len() == 100_000
                            && line_lengths.iter().all(|&len| len == 1024),
                        "{path} is unreadable and should carry the conventional table \
                         (100000 lines of 1024); got {} line(s), first {:?}",
                        line_lengths.len(),
                        &line_lengths[..line_lengths.len().min(4)]
                    );
                }
                all_tables.push(line_lengths);
            }
            let gone_id = gone_id.expect("gone.py has a paths.dat record");

            let decoder = codetracer_trace_reader::global_position_decoder::GlobalPositionDecoder::from_line_lengths(all_tables);
            let mut steps =
                codetracer_trace_reader::step_stream_reader::StepStreamReader::open(&mut reader)
                    .expect("open steps")
                    .expect("the trace has a step stream");
            let far_columns: Vec<u32> = steps
                .read_all()
                .expect("read steps")
                .into_iter()
                .filter_map(|record| match record {
                    codetracer_trace_writer::step_stream::StepStreamRecord::Step {
                        global_line_index,
                    } => Some(global_line_index),
                    codetracer_trace_writer::step_stream::StepStreamRecord::DeltaColumn {
                        global_position_index,
                        ..
                    } => Some(global_position_index),
                    _ => None,
                })
                .map(|position| {
                    decoder
                        .decode_global_position_index(position)
                        .expect("decode step")
                })
                .filter(|pos| pos.file == gone_id && pos.line == far_lineno)
                .map(|pos| pos.column)
                .collect();
            assert_eq!(
                far_columns.last().copied(),
                Some(1024),
                "the step on gone.py line {far_lineno} (store at column {store_column}) should be recorded at column 1024; \
                 columns on that line: {far_columns:?}"
            );
        });
    }

    /// `internal-files.md` §"`paths.dat` Layout A": a file that exists but
    /// holds nothing is registered with the table `[1]`, so its size is not
    /// zero.
    #[test]
    fn an_empty_module_is_registered_with_one_position() {
        Python::with_gil(|py| {
            reset_policy(py);
            ensure_test_module(py);

            let project = tempfile::tempdir().expect("project dir");
            let project_root = project.path();
            let package_dir = project_root.join("ct_empty_pkg");
            fs::create_dir(&package_dir).expect("create package");
            let empty_module = package_dir.join("__init__.py");
            fs::write(&empty_module, "").expect("write empty module");

            // A real sys.monitoring session, so the import reports the empty
            // module's code object as sys.monitoring does.
            let script_path = project_root.join("main.py");
            fs::write(&script_path, "import ct_empty_pkg\nvalue = 1\n").expect("write script");
            let outputs_dir = tempfile::tempdir().expect("outputs dir");
            // The session names the program, and its container, after argv[0].
            let argv = CString::new(format!(
                "import sys\nsys.argv = [r\"{}\"]",
                script_path.display()
            ))
            .expect("argv code");
            py.run(argv.as_c_str(), None, None).expect("set sys.argv");
            crate::session::start_tracing(
                &outputs_dir.path().to_string_lossy(),
                "ctfs",
                Some(&script_path.to_string_lossy()),
                None,
                None,
            )
            .expect("start tracing");
            let run_code = format!(
                "import runpy, sys\nsys.path.insert(0, r\"{}\")\nrunpy.run_path(r\"{}\", run_name='__main__')",
                project_root.display(),
                script_path.display()
            );
            let run_code_c = CString::new(run_code).expect("script contains nul byte");
            let run = py.run(run_code_c.as_c_str(), None, None);
            crate::session::stop_tracing(Some(0)).expect("stop tracing");
            run.expect("execute script");

            let container = recorded_container(outputs_dir.path());
            let mut reader = codetracer_ctfs::CtfsReader::open(&container)
                .unwrap_or_else(|err| panic!("open {}: {err:?}", container.display()));
            let tables =
                codetracer_trace_reader::interning_tables_reader::InterningTablesReader::open(
                    &mut reader,
                )
                .expect("read interning tables")
                .expect("the trace has interning tables");
            let recorded: Vec<(String, Vec<u32>)> = (0..tables.path_count() as u64)
                .map(|id| {
                    (
                        tables.path_str(id).expect("path"),
                        tables.path_line_lengths(id).expect("line lengths"),
                    )
                })
                .collect();
            let empty = recorded
                .iter()
                .find(|(path, _)| Path::new(path) == empty_module)
                .unwrap_or_else(|| {
                    panic!("the empty module has no paths.dat record: {recorded:?}")
                });
            assert_eq!(empty.1, vec![1], "the empty module's line table");
        });
    }

    /// Run `code` under a real sys.monitoring session that writes a CTFS
    /// trace into `outputs`, with `argv0` as the program and `filters` as
    /// the trace-filter chain, and return every `paths.dat` record.
    fn record_session_paths(
        py: Python<'_>,
        outputs: &Path,
        argv0: &str,
        filters: Option<Vec<String>>,
        code: &str,
    ) -> Vec<(String, Vec<u32>)> {
        let argv = CString::new(format!("import sys\nsys.argv = [{argv0:?}]")).expect("argv code");
        py.run(argv.as_c_str(), None, None).expect("set sys.argv");
        crate::session::start_tracing(&outputs.to_string_lossy(), "ctfs", None, filters, None)
            .expect("start tracing");
        let code_c = CString::new(code).expect("code contains nul byte");
        let run = py.run(code_c.as_c_str(), None, None);
        crate::session::stop_tracing(Some(0)).expect("stop tracing");
        run.expect("execute code");

        let container = recorded_container(outputs);
        let mut reader = codetracer_ctfs::CtfsReader::open(&container)
            .unwrap_or_else(|err| panic!("open {}: {err:?}", container.display()));
        let tables = codetracer_trace_reader::interning_tables_reader::InterningTablesReader::open(
            &mut reader,
        )
        .expect("read interning tables")
        .expect("the trace has interning tables");
        (0..tables.path_count() as u64)
            .map(|id| {
                (
                    tables.path_str(id).expect("path"),
                    tables.path_line_lengths(id).expect("line lengths"),
                )
            })
            .collect()
    }

    /// Output written by untraced code in a file, before any traced step on
    /// the thread, names that file. The file's first mention must still
    /// carry its real line table: the writer fixes a table at the first
    /// mention and refuses a different one later.
    #[test]
    fn a_file_first_named_by_output_keeps_its_real_line_table() {
        Python::with_gil(|py| {
            reset_policy(py);
            policy::configure_policy_py(
                None,
                None,
                None,
                None,
                None,
                None,
                Some(true),
                None,
                None,
                None,
            )
            .expect("capture output through the line proxies");
            let project = tempfile::tempdir().expect("project dir");
            let root = project.path();
            let helper_path = root.join("ct_io_helper.py");
            // Output capture names the file of the frame two levels above the
            // write, so the print sits three calls deep inside the helper.
            let helper_source = "def quiet():\n    _relay()\n\n\ndef _relay():\n    _say()\n\n\ndef _say():\n    print('from an untraced function')\n\n\ndef loud():\n    value = 1\n    return value\n";
            fs::write(&helper_path, helper_source).expect("write helper");
            fs::write(root.join("ct_io_traced.py"), "marker = 1\n").expect("write traced module");
            let launcher = root.join("launcher.py");
            fs::write(&launcher, "pass\n").expect("write launcher");
            let filter = root.join("filter.toml");
            fs::write(
                &filter,
                r#"
[meta]
name = "untraced-quiet"
version = 1

[scope]
default_exec = "trace"
default_value_action = "allow"

[[scope.rules]]
selector = "pkg:ct_io_helper"
exec = "skip"

[[scope.rules]]
selector = "obj:ct_io_helper.loud"
exec = "trace"
"#,
            )
            .expect("write filter");

            let outputs = tempfile::tempdir().expect("outputs dir");
            let code = format!(
                "import sys\nsys.path.insert(0, r\"{root}\")\nimport ct_io_helper\nct_io_helper.quiet()\nimport ct_io_traced\nct_io_helper.loud()\n",
                root = root.display()
            );
            let recorded = record_session_paths(
                py,
                outputs.path(),
                &launcher.to_string_lossy(),
                Some(vec![filter.to_string_lossy().into_owned()]),
                &code,
            );
            let helper = recorded
                .iter()
                .find(|(path, _)| Path::new(path) == helper_path)
                .unwrap_or_else(|| panic!("ct_io_helper.py has no paths.dat record: {recorded:?}"));
            let want = source_line_lengths(helper_source);
            assert!(
                helper.1 == want,
                "ct_io_helper.py should carry its source's table {want:?}; got {} line(s) starting {:?}",
                helper.1.len(),
                &helper.1[..helper.1.len().min(4)]
            );
        });
    }

    /// A program named by a relative argv[0] and the absolute filename
    /// Python gives its code are one file, with one `paths.dat` record.
    #[test]
    fn a_relative_program_path_and_its_code_filename_are_one_path() {
        Python::with_gil(|py| {
            reset_policy(py);
            let project = tempfile::tempdir().expect("project dir");
            let root = project.path();
            let script = root.join("ct_rel_main.py");
            fs::write(&script, "value = 1\nvalue += 1\n").expect("write script");
            let chdir = CString::new(format!("import os\nos.chdir(r\"{}\")", root.display()))
                .expect("chdir code");
            py.run(chdir.as_c_str(), None, None).expect("chdir");

            let outputs = tempfile::tempdir().expect("outputs dir");
            let code = format!(
                "import runpy\nrunpy.run_path(r\"{}\", run_name='__main__')\n",
                script.display()
            );
            let recorded = record_session_paths(py, outputs.path(), "ct_rel_main.py", None, &code);
            let named: Vec<&(String, Vec<u32>)> = recorded
                .iter()
                .filter(|(path, _)| Path::new(path).file_name() == script.file_name())
                .collect();
            assert_eq!(
                named.len(),
                1,
                "ct_rel_main.py should have one paths.dat record: {recorded:?}"
            );
            assert_eq!(
                Path::new(&named[0].0),
                script,
                "the record names the absolute path"
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
        let final_coroutine_step = coroutine_steps.last().expect("at least one coroutine step");
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
                Some(false),
                Some(false),
            )
            .expect("enable require_trace policy");

            let script_dir = tempfile::tempdir().expect("script dir");
            let program_path = script_dir.path().join("program.py");
            std::fs::write(&program_path, "print('hi')\n").expect("write program");

            let outputs_dir = tempfile::tempdir().expect("outputs dir");
            let outputs =
                TraceOutputPaths::new(outputs_dir.path(), TraceEventsFileFormat::BinaryV0, "program.py");

            let mut tracer = RuntimeTracer::new(
                program_path.to_string_lossy().as_ref(),
                &[],
                TraceEventsFileFormat::BinaryV0,
                None,
                None,
                false,
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

    /// The `.ct` containers in `dir`: what a failed recording leaves behind.
    fn ct_containers(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .expect("read outputs dir")
            .map(|entry| entry.expect("dir entry").path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "ct"))
            .collect()
    }

    /// Begin a real CTFS recording of `program.py` into a fresh directory and
    /// mark it failed, so `finish` has to decide about the container the
    /// writer actually created.
    fn failed_ctfs_recording() -> (tempfile::TempDir, tempfile::TempDir, RuntimeTracer) {
        let script_dir = tempfile::tempdir().expect("script dir");
        let program_path = script_dir.path().join("program.py");
        std::fs::write(&program_path, "print('hi')\n").expect("write program");

        let outputs_dir = tempfile::tempdir().expect("outputs dir");
        let program = program_path.to_string_lossy().into_owned();
        let outputs = TraceOutputPaths::new(outputs_dir.path(), TraceEventsFileFormat::Ctfs, &program);

        let mut tracer = RuntimeTracer::new(
            &program,
            &[],
            TraceEventsFileFormat::Ctfs,
            None,
            None,
            false,
        );
        tracer.begin(&outputs, 1).expect("begin tracer");
        assert_eq!(
            ct_containers(outputs_dir.path()),
            vec![outputs_dir.path().join("program.ct")],
            "the writer must have created the recording's container"
        );
        tracer.mark_failure();
        (script_dir, outputs_dir, tracer)
    }

    #[test]
    fn finish_removes_partial_outputs_when_policy_forbids_keep() {
        Python::with_gil(|py| {
            reset_policy(py);

            let (_script_dir, outputs_dir, mut tracer) = failed_ctfs_recording();
            tracer.finish(py).expect("finish after failure");

            assert_eq!(
                ct_containers(outputs_dir.path()),
                Vec::<PathBuf>::new(),
                "a failed recording's container must be removed when the policy does not keep \
                 partial traces"
            );
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
                Some(false),
                Some(false),
            )
            .expect("enable keep_partial policy");

            let (_script_dir, outputs_dir, mut tracer) = failed_ctfs_recording();
            tracer.finish(py).expect("finish after failure");

            assert_eq!(
                ct_containers(outputs_dir.path()),
                vec![outputs_dir.path().join("program.ct")],
                "a failed recording's container must be kept when the policy keeps partial traces"
            );

            reset_policy(py);
        });
    }

    // ------------------------------------------------------------------
    // M15: Python recorder Assignment events
    // ------------------------------------------------------------------

    /// Resolve a `VariableId` back to its source name by walking the
    /// `VariableName` events emitted at the point the id was minted (the
    /// NonStreamingTraceWriter assigns ids in the order names are first
    /// observed; see `codetracer_trace_writer_nim::NonStreamingTraceWriter
    /// ::ensure_variable_id`).
    fn variable_name_for(
        events: &[TraceLowLevelEvent],
        id: codetracer_trace_types::VariableId,
    ) -> Option<String> {
        let mut counter = 0usize;
        for event in events {
            if let TraceLowLevelEvent::VariableName(name) = event {
                if counter == id.0 {
                    return Some(name.clone());
                }
                counter += 1;
            }
        }
        None
    }

    /// Convenience: collect every `(target_name, RValue)` pair seen in
    /// `events`, in order.
    fn collect_assignments(
        events: &[TraceLowLevelEvent],
    ) -> Vec<(String, codetracer_trace_types::RValue)> {
        events
            .iter()
            .filter_map(|e| match e {
                TraceLowLevelEvent::Assignment(rec) => Some((
                    variable_name_for(events, rec.to).unwrap_or_else(|| format!("?{}", rec.to.0)),
                    rec.from.clone(),
                )),
                _ => None,
            })
            .collect()
    }

    /// Convenience: collect every name that received a `BindVariable` event.
    fn collect_bind_variable_names(events: &[TraceLowLevelEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|e| match e {
                TraceLowLevelEvent::BindVariable(rec) => {
                    Some(variable_name_for(events, rec.variable_id).unwrap_or_default())
                }
                _ => None,
            })
            .collect()
    }

    /// Capture the buffered events from a script-driven trace.
    ///
    /// Like `run_traced_script` but exposes the raw events vector instead of
    /// the simplified snapshots — needed so the M15 tests can inspect
    /// `Assignment`, `BindVariable`, and the column on `StepRecord`.
    fn run_traced_script_events(body: &str) -> Vec<TraceLowLevelEvent> {
        Python::with_gil(|py| {
            let mut tracer = RuntimeTracer::new(
                "test.py",
                &[],
                TraceEventsFileFormat::BinaryV0,
                None,
                None,
                false,
            );
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
            tracer.writer.events().to_vec()
        })
    }

    #[test]
    fn test_python_recorder_emits_assignment_for_simple_assignment() {
        // `a = 10` must surface as Assignment { from: Literal }. The trailing
        // `snapshot()` triggers an extra on_line event so the Assignment for
        // the previous line gets flushed (see the on_line emit-on-previous
        // ordering in `events.rs::on_line`).
        let body = r#"
a = 10
snapshot()
"#;
        let events = run_traced_script_events(body);
        let assignments = collect_assignments(&events);
        let a_assign = assignments
            .iter()
            .find(|(name, _)| name == "a")
            .unwrap_or_else(|| {
                panic!(
                    "expected Assignment for `a`, got assignments={:?}",
                    assignments
                )
            });
        assert!(
            matches!(a_assign.1, codetracer_trace_types::RValue::Literal),
            "expected RValue::Literal for `a = 10`, got {:?}",
            a_assign.1
        );

        let binds = collect_bind_variable_names(&events);
        assert!(
            binds.contains(&"a".to_string()),
            "expected BindVariable for `a`, got {:?}",
            binds
        );
    }

    #[test]
    fn test_python_recorder_emits_assignment_for_local_copy() {
        // `b = a` must surface as Assignment { from: Simple(var_id(a)) }.
        let body = r#"
a = 10
b = a
snapshot()
"#;
        let events = run_traced_script_events(body);
        let assignments = collect_assignments(&events);
        let b_assign = assignments
            .iter()
            .find(|(name, _)| name == "b")
            .unwrap_or_else(|| {
                panic!(
                    "expected Assignment for `b`, got assignments={:?}",
                    assignments
                )
            });
        match &b_assign.1 {
            codetracer_trace_types::RValue::Simple(id) => {
                let src = variable_name_for(&events, *id).unwrap_or_default();
                assert_eq!(
                    src, "a",
                    "expected RValue::Simple(a), got Simple({:?})",
                    src
                );
            }
            other => panic!("expected RValue::Simple, got {:?}", other),
        }
    }

    #[test]
    fn test_python_recorder_emits_function_return_rvalue() {
        // `result = foo()` must surface as Assignment { from: FunctionReturn{call_key} }.
        //
        // The test harness drives the tracer manually via PRELUDE helpers:
        // - `start_call()` calls into on_py_start so the writer registers a
        //   CallRecord and `last_call_key` advances. We invoke it from inside
        //   foo() so it fires when foo executes.
        // - The final `snapshot()` flushes the per-line Assignment emit pass.
        //
        // The bytecode reconstructor recognises `result = foo()` as a CALL
        // pattern regardless of whether the harness actually invoked
        // on_py_start; the `last_call_key` only affects which CallKey value
        // gets stamped on the RValue::FunctionReturn variant.
        let body = r#"
def foo():
    start_call()
    return 42

result = foo()
snapshot()
"#;
        let events = run_traced_script_events(body);
        let assignments = collect_assignments(&events);
        let result_assign = assignments
            .iter()
            .find(|(name, _)| name == "result")
            .unwrap_or_else(|| {
                panic!(
                    "expected Assignment for `result`, got assignments={:?}",
                    assignments
                )
            });
        assert!(
            matches!(
                result_assign.1,
                codetracer_trace_types::RValue::FunctionReturn { .. }
            ),
            "expected RValue::FunctionReturn for `result = foo()`, got {:?}",
            result_assign.1
        );
    }

    #[test]
    fn test_python_recorder_emits_destructuring_assignments() {
        // `a, b = pair` must surface as TWO Assignment events with
        // RValue::IndexAccess { receiver: pair, index: i }.
        let body = r#"
pair = (11, 22)
a, b = pair
snapshot()
"#;
        let events = run_traced_script_events(body);
        let assignments = collect_assignments(&events);
        let a_assign = assignments
            .iter()
            .find(|(name, _)| name == "a")
            .unwrap_or_else(|| {
                panic!(
                    "expected Assignment for `a`, got assignments={:?}",
                    assignments
                )
            });
        let b_assign = assignments
            .iter()
            .find(|(name, _)| name == "b")
            .unwrap_or_else(|| {
                panic!(
                    "expected Assignment for `b`, got assignments={:?}",
                    assignments
                )
            });
        match &a_assign.1 {
            codetracer_trace_types::RValue::IndexAccess { receiver, index } => {
                let src = variable_name_for(&events, *receiver).unwrap_or_default();
                assert_eq!(src, "pair", "a: receiver expected pair, got {:?}", src);
                assert_eq!(*index, 0, "a: expected index 0, got {}", index);
            }
            other => panic!("expected RValue::IndexAccess for a, got {:?}", other),
        }
        match &b_assign.1 {
            codetracer_trace_types::RValue::IndexAccess { receiver, index } => {
                let src = variable_name_for(&events, *receiver).unwrap_or_default();
                assert_eq!(src, "pair", "b: receiver expected pair, got {:?}", src);
                assert_eq!(*index, 1, "b: expected index 1, got {}", index);
            }
            other => panic!("expected RValue::IndexAccess for b, got {:?}", other),
        }
    }

    #[test]
    fn test_python_recorder_emits_step_records_for_column_carrying_lines() {
        // P1.1/P1.2 (was: M14/M15 `StepRecord.column` test).  The
        // canonical CTFS column-encoding path emits column-only
        // `DeltaColumn` (tag 0x07) events on the multi-stream backend,
        // not a column field on `StepRecord`.  Column-aware acceptance
        // lives in `tests/python/test_column_aware_steps.py`, which
        // drives a full CTFS round-trip via the recorder CLI.
        //
        // This unit test now keeps a weaker but still useful smoke
        // assertion against the JSON test-double backend: a script with
        // a STORE on every body line produces at least one `Step`
        // event per line, proving the extraction-and-emission glue is
        // intact for the non-column-aware path.  The JSON test double
        // (`NonStreamingTraceWriter`) drops column data by design — see
        // its `register_step_with_column` impl.
        let body = r#"
a = snap(10)
b = snap(20)
"#;
        let events = run_traced_script_events(body);
        let step_count = events
            .iter()
            .filter(|e| matches!(e, TraceLowLevelEvent::Step(_)))
            .count();
        assert!(
            step_count >= 2,
            "expected >=2 Step events (one per body line), got {} in {:?}",
            step_count,
            events
                .iter()
                .filter_map(|e| match e {
                    TraceLowLevelEvent::Step(rec) => Some(rec),
                    _ => None,
                })
                .collect::<Vec<_>>()
        );
    }

    /// M15 verification 6: Path A confidence-1.0 is a property of the
    /// db-backend's classifier, not the recorder. The recorder satisfies its
    /// half of the contract by ensuring the buffered event stream contains
    /// `Assignment` events for every store on a real-frame line. This test
    /// asserts that property: every `b = a`-style local-copy assignment
    /// emits an Assignment event with RValue::Simple, which is exactly what
    /// triggers Path A activation in `trace_processor.rs`. The db-backend
    /// then upgrades the per-hop confidence to 1.0 (spec §6.1.5).
    #[test]
    fn test_origin_chain_path_a_confidence_one() {
        let body = r#"
a = 10
b = a
c = b
snapshot()
"#;
        let events = run_traced_script_events(body);
        let assignments = collect_assignments(&events);

        // b = a -> Simple(a)
        let b_assign = assignments
            .iter()
            .find(|(name, _)| name == "b")
            .expect("Assignment(b) present");
        assert!(
            matches!(b_assign.1, codetracer_trace_types::RValue::Simple(_)),
            "Path A: `b = a` needs Simple, got {:?}",
            b_assign.1
        );

        // c = b -> Simple(b)
        let c_assign = assignments
            .iter()
            .find(|(name, _)| name == "c")
            .expect("Assignment(c) present");
        match &c_assign.1 {
            codetracer_trace_types::RValue::Simple(id) => {
                let src = variable_name_for(&events, *id).unwrap_or_default();
                assert_eq!(
                    src, "b",
                    "Path A: `c = b` source should be b, got {:?}",
                    src
                );
            }
            other => panic!("Path A: `c = b` needs Simple, got {:?}", other),
        }

        // The chain a (Literal) -> b (Simple a) -> c (Simple b) is
        // wholly Path A reconstructible from the recorder events alone.
        let a_assign = assignments
            .iter()
            .find(|(name, _)| name == "a")
            .expect("Assignment(a) present");
        assert!(
            matches!(a_assign.1, codetracer_trace_types::RValue::Literal),
            "Path A chain root must be Literal, got {:?}",
            a_assign.1
        );
    }
}
