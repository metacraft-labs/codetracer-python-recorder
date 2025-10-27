use super::events::suppress_events;
use super::filtering::{FilterCoordinator, TraceDecision};
use super::io::IoCoordinator;
use super::lifecycle::LifecycleController;
use crate::code_object::CodeObjectWrapper;
use crate::ffi;
use crate::module_identity::{ModuleIdentityCache, ModuleNameHints};
use crate::policy::RecorderPolicy;
use crate::runtime::io_capture::{IoCaptureSettings, ScopedMuteIoCapture};
use crate::runtime::line_snapshots::LineSnapshotStore;
use crate::runtime::output_paths::TraceOutputPaths;
use crate::runtime::value_encoder::encode_value;
use crate::trace_filter::engine::TraceFilterEngine;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyInt, PyString};
use runtime_tracing::NonStreamingTraceWriter;
use runtime_tracing::{Line, TraceEventsFileFormat, TraceWriter};
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
}

/// Minimal runtime tracer that maps Python sys.monitoring events to
/// runtime_tracing writer operations.
pub struct RuntimeTracer {
    pub(super) writer: NonStreamingTraceWriter,
    pub(super) format: TraceEventsFileFormat,
    pub(super) lifecycle: LifecycleController,
    pub(super) function_ids: HashMap<usize, runtime_tracing::FunctionId>,
    pub(super) io: IoCoordinator,
    pub(super) filter: FilterCoordinator,
    pub(super) module_names: ModuleIdentityCache,
    session_exit: SessionExitState,
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
        let lifecycle = LifecycleController::new(program, activation_path);
        Self {
            writer,
            format,
            lifecycle,
            function_ids: HashMap::new(),
            io: IoCoordinator::new(),
            filter: FilterCoordinator::new(trace_filter),
            module_names: ModuleIdentityCache::new(),
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
        if self.io.flush_before_step(thread_id, &mut self.writer) {
            self.mark_event();
        }
    }

    pub(super) fn flush_pending_io(&mut self) {
        if self.io.flush_all(&mut self.writer) {
            self.mark_event();
        }
    }

    pub(super) fn emit_session_exit(&mut self, py: Python<'_>) {
        if self.session_exit.is_emitted() {
            return;
        }

        self.flush_pending_io();
        let value = self.session_exit.as_bound(py);
        let record = encode_value(py, &mut self.writer, &value);
        TraceWriter::register_return(&mut self.writer, record);
        self.session_exit.mark_emitted();
    }

    /// Configure output files and write initial metadata records.
    pub fn begin(&mut self, outputs: &TraceOutputPaths, start_line: u32) -> PyResult<()> {
        self.lifecycle
            .begin(&mut self.writer, outputs, start_line)
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

    pub(super) fn ensure_function_id(
        &mut self,
        py: Python<'_>,
        code: &CodeObjectWrapper,
    ) -> PyResult<runtime_tracing::FunctionId> {
        if let Some(fid) = self.function_ids.get(&code.id()) {
            return Ok(*fid);
        }
        let name = self.function_name(py, code)?;
        let filename = code.filename(py)?;
        let first_line = code.first_line(py)?;
        let function_id = TraceWriter::ensure_function_id(
            &mut self.writer,
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
        let resolution = self.filter.cached_resolution(code.id());
        if let Some(resolution) = resolution.as_ref() {
            let hints = ModuleNameHints {
                preferred: resolution.module_name(),
                relative_path: resolution.relative_path(),
                absolute_path: resolution.absolute_path(),
                globals_name: None,
            };
            self.module_names.resolve_for_code(py, code, hints)
        } else {
            self.module_names
                .resolve_for_code(py, code, ModuleNameHints::default())
        }
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
    use pyo3::types::{PyAny, PyCode, PyModule};
    use pyo3::wrap_pyfunction;
    use runtime_tracing::{FullValueRecord, StepRecord, TraceLowLevelEvent, ValueRecord};
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
            assert_no_variable(&snapshots, "internal");

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

            let tracer =
                RuntimeTracer::new("runner.py", &[], TraceEventsFileFormat::Json, None, None);

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
                    .expect("execute dropper script");
            }

            let mut variable_names: Vec<String> = Vec::new();
            let mut return_values: Vec<ValueRecord> = Vec::new();
            for event in &tracer.writer.events {
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
            assert_eq!(return_values.len(), 1, "return event should remain balanced");
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
                    .expect("execute classes script");
            }

            let mut call_count = 0usize;
            let mut return_count = 0usize;
            for event in &tracer.writer.events {
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
            let outputs = TraceOutputPaths::new(outputs_dir.path(), TraceEventsFileFormat::Json);

            let mut tracer = RuntimeTracer::new(
                program_path.to_string_lossy().as_ref(),
                &[],
                TraceEventsFileFormat::Json,
                None,
                None,
            );
            tracer.begin(&outputs, 1).expect("begin tracer");
            tracer.record_exit_status(Some(7));

            tracer.finish(py).expect("finish tracer");

            let mut exit_value: Option<ValueRecord> = None;
            for event in &tracer.writer.events {
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
            let value_drops = stats
                .get("value_drops")
                .and_then(|value| value.as_object())
                .expect("value_drops object");
            assert_eq!(
                value_drops.get("argument").and_then(|v| v.as_u64()),
                Some(0)
            );
            assert_eq!(value_drops.get("local").and_then(|v| v.as_u64()), Some(1));
            assert_eq!(value_drops.get("global").and_then(|v| v.as_u64()), Some(0));
            assert_eq!(value_drops.get("return").and_then(|v| v.as_u64()), Some(0));
            assert_eq!(
                value_drops.get("attribute").and_then(|v| v.as_u64()),
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
