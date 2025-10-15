//! IO stream proxies that attribute Python stdio activity without breaking passthrough output.

use crate::runtime::line_snapshots::FrameId;
use pyo3::exceptions::PyStopIteration;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyAnyMethods, PyList, PyTuple};
use pyo3::IntoPyObject;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};
use std::time::Instant;

/// Distinguishes the proxied streams.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoStream {
    Stdout,
    Stderr,
    Stdin,
}

impl fmt::Display for IoStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IoStream::Stdout => write!(f, "stdout"),
            IoStream::Stderr => write!(f, "stderr"),
            IoStream::Stdin => write!(f, "stdin"),
        }
    }
}

/// Operations surfaced by the proxies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoOperation {
    Write,
    Writelines,
    Flush,
    Read,
    ReadLine,
    ReadLines,
    ReadInto,
}

/// Raw proxy payload collected during Stage 1.
#[derive(Clone, Debug)]
pub struct ProxyEvent {
    pub stream: IoStream,
    pub operation: IoOperation,
    pub payload: Vec<u8>,
    pub thread_id: ThreadId,
    pub timestamp: Instant,
    pub frame_id: Option<FrameId>,
}

/// Sink for proxy events. Later stages swap in a real writer-backed implementation.
pub trait ProxySink: Send + Sync + 'static {
    fn record(&self, py: Python<'_>, event: ProxyEvent);
}

/// No-op sink for scenarios where IO capture is disabled but proxies must install.
pub struct NullSink;

impl ProxySink for NullSink {
    fn record(&self, _py: Python<'_>, _event: ProxyEvent) {}
}


// Thread-local guard to prevent recursion when sinks write back to the proxies.
//
// Reentrancy hazard and rationale:
//
// - ProxySink::record implementations may perform Python I/O (e.g. sys.stdout.write or sys.stderr.write)
//   while we are already inside a proxied I/O call (stdout/stderr writes or stdin reads).
// - Without a guard, those sink-triggered writes would re-enter these proxies, which would call the sink
//   again, and so on. That can cause infinite recursion, stack overflow, and duplicate event capture.
//
// How we avoid it:
//
// - On first entry into a proxied I/O method we set a thread-local flag.
// - While that flag is set, we still forward I/O to the original Python object, but we skip recording.
// - This allows sink-triggered I/O to pass through to Python without being captured, breaking the cycle.
//
// See test reentrant_sink_does_not_loop for coverage.
thread_local! {
    static IN_PROXY_CALLBACK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn enter_reentrancy_guard() -> bool {
    IN_PROXY_CALLBACK.with(|flag| {
        if flag.get() {
            false
        } else {
            flag.set(true);
            true
        }
    })
}

fn exit_reentrancy_guard(entered: bool) {
    if entered {
        IN_PROXY_CALLBACK.with(|flag| flag.set(false));
    }
}

fn build_iterator_list(iterable: &Bound<'_, PyAny>) -> PyResult<(Vec<String>, Py<PyList>)> {
    let mut iterator = iterable.try_iter()?;
    let mut captured = Vec::new();
    while let Some(item) = iterator.next() {
        let obj = item?;
        captured.push(obj.extract::<String>()?);
    }
    let py_list = PyList::new(iterable.py(), &captured)?.unbind();
    Ok((captured, py_list))
}

fn buffer_snapshot(buffer: &Bound<'_, PyAny>) -> Option<Vec<u8>> {
    buffer
        .call_method0("__bytes__")
        .ok()
        .and_then(|obj| obj.extract::<Vec<u8>>().ok())
}

fn current_thread_id() -> ThreadId {
    thread::current().id()
}

fn now() -> Instant {
    Instant::now()
}

struct OutputProxy {
    original: PyObject,
    sink: Arc<dyn ProxySink>,
    stream: IoStream,
}

impl OutputProxy {
    fn new(original: PyObject, sink: Arc<dyn ProxySink>, stream: IoStream) -> Self {
        Self {
            original,
            sink,
            stream,
        }
    }

    fn record(&self, py: Python<'_>, operation: IoOperation, payload: Vec<u8>) {
        let event = ProxyEvent {
            stream: self.stream,
            operation,
            payload,
            thread_id: current_thread_id(),
            timestamp: now(),
            frame_id: None,
        };
        self.sink.record(py, event);
    }

    fn call_method1<'py, A>(
        &self,
        py: Python<'py>,
        method: &str,
        args: A,
        payload: Option<Vec<u8>>,
        operation: IoOperation,
    ) -> PyResult<Py<PyAny>>
    where
        A: IntoPyObject<'py, Target = PyTuple>,
    {
        let entered = enter_reentrancy_guard();
        let result = self
            .original
            .call_method1(py, method, args)
            .map(|value| value.into());
        if entered {
            if let (Ok(_), Some(data)) = (&result, payload) {
                self.record(py, operation, data);
            }
        }
        exit_reentrancy_guard(entered);
        result
    }

    fn passthrough<'py, A>(&self, py: Python<'py>, method: &str, args: A) -> PyResult<Py<PyAny>>
    where
        A: IntoPyObject<'py, Target = PyTuple>,
    {
        self.original
            .call_method1(py, method, args)
            .map(|value| value.into())
    }
}

#[pyclass(module = "codetracer_python_recorder.runtime")]
pub struct LineAwareStdout {
    inner: OutputProxy,
}

impl LineAwareStdout {
    pub fn new(original: PyObject, sink: Arc<dyn ProxySink>) -> Self {
        Self {
            inner: OutputProxy::new(original, sink, IoStream::Stdout),
        }
    }
}

#[pymethods]
impl LineAwareStdout {
    fn write(&self, py: Python<'_>, text: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let captured = text.extract::<String>()?.into_bytes();
        let args = (text.clone().unbind(),);
        self.inner
            .call_method1(py, "write", args, Some(captured), IoOperation::Write)
    }

    fn writelines(&self, py: Python<'_>, lines: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let (captured, replay) = build_iterator_list(lines)?;
        let payload = captured.join("").into_bytes();
        self.inner.call_method1(
            py,
            "writelines",
            (replay.clone_ref(py),),
            Some(payload),
            IoOperation::Writelines,
        )
    }

    fn flush(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.inner
            .call_method1(py, "flush", (), None, IoOperation::Flush)
    }

    fn fileno(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.inner.passthrough(py, "fileno", ())
    }

    fn isatty(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.inner.passthrough(py, "isatty", ())
    }

    fn close(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.inner.passthrough(py, "close", ())
    }

    #[getter]
    fn encoding(&self, py: Python<'_>) -> PyResult<PyObject> {
        self.inner
            .original
            .bind(py)
            .getattr("encoding")
            .map(|obj| obj.unbind())
    }

    #[getter]
    fn errors(&self, py: Python<'_>) -> PyResult<PyObject> {
        self.inner
            .original
            .bind(py)
            .getattr("errors")
            .map(|obj| obj.unbind())
    }

    #[getter]
    fn buffer(&self, py: Python<'_>) -> PyResult<PyObject> {
        self.inner
            .original
            .bind(py)
            .getattr("buffer")
            .map(|obj| obj.unbind())
    }

    fn __getattr__(&self, py: Python<'_>, name: &str) -> PyResult<PyObject> {
        self.inner
            .original
            .bind(py)
            .getattr(name)
            .map(|obj| obj.unbind())
    }
}

#[pyclass(module = "codetracer_python_recorder.runtime")]
pub struct LineAwareStderr {
    inner: OutputProxy,
}

impl LineAwareStderr {
    pub fn new(original: PyObject, sink: Arc<dyn ProxySink>) -> Self {
        Self {
            inner: OutputProxy::new(original, sink, IoStream::Stderr),
        }
    }
}

#[pymethods]
impl LineAwareStderr {
    fn write(&self, py: Python<'_>, text: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let captured = text.extract::<String>()?.into_bytes();
        let args = (text.clone().unbind(),);
        self.inner
            .call_method1(py, "write", args, Some(captured), IoOperation::Write)
    }

    fn writelines(&self, py: Python<'_>, lines: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let (captured, replay) = build_iterator_list(lines)?;
        let payload = captured.join("").into_bytes();
        self.inner.call_method1(
            py,
            "writelines",
            (replay.clone_ref(py),),
            Some(payload),
            IoOperation::Writelines,
        )
    }

    fn flush(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.inner
            .call_method1(py, "flush", (), None, IoOperation::Flush)
    }

    fn fileno(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.inner.passthrough(py, "fileno", ())
    }

    fn isatty(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.inner.passthrough(py, "isatty", ())
    }

    fn close(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.inner.passthrough(py, "close", ())
    }

    #[getter]
    fn encoding(&self, py: Python<'_>) -> PyResult<PyObject> {
        self.inner
            .original
            .bind(py)
            .getattr("encoding")
            .map(|obj| obj.unbind())
    }

    #[getter]
    fn errors(&self, py: Python<'_>) -> PyResult<PyObject> {
        self.inner
            .original
            .bind(py)
            .getattr("errors")
            .map(|obj| obj.unbind())
    }

    #[getter]
    fn buffer(&self, py: Python<'_>) -> PyResult<PyObject> {
        self.inner
            .original
            .bind(py)
            .getattr("buffer")
            .map(|obj| obj.unbind())
    }

    fn __getattr__(&self, py: Python<'_>, name: &str) -> PyResult<PyObject> {
        self.inner
            .original
            .bind(py)
            .getattr(name)
            .map(|obj| obj.unbind())
    }
}

#[pyclass(module = "codetracer_python_recorder.runtime")]
pub struct LineAwareStdin {
    original: PyObject,
    sink: Arc<dyn ProxySink>,
}

impl LineAwareStdin {
    pub fn new(original: PyObject, sink: Arc<dyn ProxySink>) -> Self {
        Self { original, sink }
    }

    fn record(&self, py: Python<'_>, operation: IoOperation, payload: Vec<u8>) {
        let event = ProxyEvent {
            stream: IoStream::Stdin,
            operation,
            payload,
            thread_id: current_thread_id(),
            timestamp: now(),
            frame_id: None,
        };
        self.sink.record(py, event);
    }
}

#[pymethods]
impl LineAwareStdin {
    #[pyo3(signature = (size=None))]
    fn read(&self, py: Python<'_>, size: Option<isize>) -> PyResult<Py<PyAny>> {
        let entered = enter_reentrancy_guard();
        let result: PyResult<Py<PyAny>> = match size {
            Some(n) => self
                .original
                .call_method1(py, "read", (n,))
                .map(|value| value.into()),
            None => self
                .original
                .call_method1(py, "read", ())
                .map(|value| value.into()),
        };
        if entered {
            if let Ok(ref obj) = result {
                let bound = obj.bind(py);
                if let Ok(text) = bound.extract::<String>() {
                    if !text.is_empty() {
                        self.record(py, IoOperation::Read, text.into_bytes());
                    }
                }
            }
        }
        exit_reentrancy_guard(entered);
        result
    }

    #[pyo3(signature = (limit=None))]
    fn readline(&self, py: Python<'_>, limit: Option<isize>) -> PyResult<Py<PyAny>> {
        let entered = enter_reentrancy_guard();
        let result: PyResult<Py<PyAny>> = match limit {
            Some(n) => self
                .original
                .call_method1(py, "readline", (n,))
                .map(|value| value.into()),
            None => self
                .original
                .call_method1(py, "readline", ())
                .map(|value| value.into()),
        };
        if entered {
            if let Ok(ref obj) = result {
                let bound = obj.bind(py);
                if let Ok(text) = bound.extract::<String>() {
                    if !text.is_empty() {
                        self.record(py, IoOperation::ReadLine, text.into_bytes());
                    }
                }
            }
        }
        exit_reentrancy_guard(entered);
        result
    }

    fn readinto(&self, py: Python<'_>, buffer: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let entered = enter_reentrancy_guard();
        let args = (buffer.clone().unbind(),);
        let result: PyResult<Py<PyAny>> = self
            .original
            .call_method1(py, "readinto", args)
            .map(|value| value.into());
        if entered {
            if let Ok(ref obj) = result {
                if let Some(mut bytes) = buffer_snapshot(buffer) {
                    if let Ok(count) = obj.bind(py).extract::<usize>() {
                        let count = count.min(bytes.len());
                        if count > 0 {
                            bytes.truncate(count);
                            self.record(py, IoOperation::ReadInto, bytes);
                        }
                    }
                }
            }
        }
        exit_reentrancy_guard(entered);
        result
    }

    fn fileno(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.original
            .call_method1(py, "fileno", ())
            .map(|value| value.into())
    }

    fn isatty(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.original
            .call_method1(py, "isatty", ())
            .map(|value| value.into())
    }

    fn close(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.original
            .call_method1(py, "close", ())
            .map(|value| value.into())
    }

    fn __iter__(slf: PyRef<Self>) -> Py<LineAwareStdin> {
        slf.into()
    }

    fn __next__(&self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
        let line = self.readline(py, None)?;
        if line.bind(py).extract::<String>()?.is_empty() {
            Err(PyStopIteration::new_err(()))
        } else {
            Ok(Some(line))
        }
    }

    fn __getattr__(&self, py: Python<'_>, name: &str) -> PyResult<PyObject> {
        self.original
            .bind(py)
            .getattr(name)
            .map(|obj| obj.unbind())
    }
}

/// Controller that installs the proxies and restores the original streams.
pub struct IoStreamProxies {
    sink: Arc<dyn ProxySink>,
    stdout_proxy: Py<LineAwareStdout>,
    stderr_proxy: Py<LineAwareStderr>,
    stdin_proxy: Py<LineAwareStdin>,
    original_stdout: PyObject,
    original_stderr: PyObject,
    original_stdin: PyObject,
    installed: bool,
}

impl IoStreamProxies {
    pub fn install(py: Python<'_>, sink: Arc<dyn ProxySink>) -> PyResult<Self> {
        let sys = py.import("sys")?;
        let stdout_original = sys.getattr("stdout")?.unbind();
        let stderr_original = sys.getattr("stderr")?.unbind();
        let stdin_original = sys.getattr("stdin")?.unbind();

        let stdout_proxy =
            Py::new(py, LineAwareStdout::new(stdout_original.clone_ref(py), sink.clone()))?;
        let stderr_proxy =
            Py::new(py, LineAwareStderr::new(stderr_original.clone_ref(py), sink.clone()))?;
        let stdin_proxy =
            Py::new(py, LineAwareStdin::new(stdin_original.clone_ref(py), sink.clone()))?;

        sys.setattr("stdout", stdout_proxy.clone_ref(py))?;
        sys.setattr("stderr", stderr_proxy.clone_ref(py))?;
        sys.setattr("stdin", stdin_proxy.clone_ref(py))?;

        Ok(Self {
            sink,
            stdout_proxy,
            stderr_proxy,
            stdin_proxy,
            original_stdout: stdout_original,
            original_stderr: stderr_original,
            original_stdin: stdin_original,
            installed: true,
        })
    }

    pub fn uninstall(&mut self, py: Python<'_>) -> PyResult<()> {
        if !self.installed {
            return Ok(());
        }
        let sys = py.import("sys")?;
        sys.setattr("stdout", &self.original_stdout)?;
        sys.setattr("stderr", &self.original_stderr)?;
        sys.setattr("stdin", &self.original_stdin)?;
        self.installed = false;
        Ok(())
    }

    pub fn sink(&self) -> Arc<dyn ProxySink> {
        self.sink.clone()
    }

    pub fn is_installed(&self) -> bool {
        self.installed
    }
}

impl Drop for IoStreamProxies {
    fn drop(&mut self) {
        Python::with_gil(|py| {
            if let Err(err) = self.uninstall(py) {
                err.print(py);
            }
        });
    }
}

/// Simple sink used by tests to assert captured payloads.
#[derive(Default)]
pub struct RecordingSink {
    events: Mutex<Vec<ProxyEvent>>,
}

impl RecordingSink {
    pub fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
        }
    }

    pub fn events(&self) -> Vec<ProxyEvent> {
        self.events.lock().expect("lock poisoned").clone()
    }
}

impl ProxySink for RecordingSink {
    fn record(&self, _py: Python<'_>, event: ProxyEvent) {
        self.events.lock().expect("lock poisoned").push(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::sync::Arc;

    fn with_string_io<F, R>(py: Python<'_>, sink: Arc<dyn ProxySink>, func: F) -> PyResult<R>
    where
        F: FnOnce(&mut IoStreamProxies) -> PyResult<R>,
    {
        let sys = py.import("sys")?;
        let io = py.import("io")?;
        let stdout_buf = io.call_method0("StringIO")?;
        let stderr_buf = io.call_method0("StringIO")?;
        let stdin_buf = io.call_method1("StringIO", ("line1\nline2\n",))?;
        sys.setattr("stdout", stdout_buf)?;
        sys.setattr("stderr", stderr_buf)?;
        sys.setattr("stdin", stdin_buf)?;

        let mut proxies = IoStreamProxies::install(py, sink)?;
        let result = func(&mut proxies)?;
        proxies.uninstall(py)?;
        Ok(result)
    }

    #[test]
    fn stdout_write_is_captured() {
        Python::with_gil(|py| {
            let sink = Arc::new(RecordingSink::new());
            with_string_io(py, sink.clone(), |_| {
                let code = CString::new("print('hello', end='')").unwrap();
                py.run(code.as_c_str(), None, None)?;
                Ok(())
            })
            .unwrap();
            let events = sink.events();
            assert!(!events.is_empty());
            assert_eq!(events[0].stream, IoStream::Stdout);
            assert_eq!(events[0].operation, IoOperation::Write);
            assert_eq!(std::str::from_utf8(&events[0].payload).unwrap(), "hello");
        });
    }

    #[test]
    fn stderr_write_is_captured() {
        Python::with_gil(|py| {
            let sink = Arc::new(RecordingSink::new());
            with_string_io(py, sink.clone(), |_| {
                let code = CString::new("import sys\nsys.stderr.write('oops')").unwrap();
                py.run(code.as_c_str(), None, None)?;
                Ok(())
            })
            .unwrap();
            let events = sink.events();
            assert!(!events.is_empty());
            assert_eq!(events[0].stream, IoStream::Stderr);
            assert_eq!(events[0].operation, IoOperation::Write);
            assert_eq!(std::str::from_utf8(&events[0].payload).unwrap(), "oops");
        });
    }

    #[test]
    fn stdin_read_is_captured() {
        Python::with_gil(|py| {
            let sink = Arc::new(RecordingSink::new());
            with_string_io(py, sink.clone(), |_| {
                let code = CString::new("import sys\n_ = sys.stdin.readline()").unwrap();
                py.run(code.as_c_str(), None, None)?;
                Ok(())
            })
            .unwrap();
            let events = sink.events();
            assert!(!events.is_empty());
            let latest = events.last().unwrap();
            assert_eq!(latest.stream, IoStream::Stdin);
            assert_eq!(latest.operation, IoOperation::ReadLine);
            assert_eq!(std::str::from_utf8(&latest.payload).unwrap(), "line1\n");
        });
    }

    #[test]
    fn reentrant_sink_does_not_loop() {
        #[derive(Default)]
        struct Reentrant {
            inner: RecordingSink,
        }

        impl ProxySink for Reentrant {
            fn record(&self, py: Python<'_>, event: ProxyEvent) {
                self.inner.record(py, event.clone());
                let _ = py
                    .import("sys")
                    .and_then(|sys| sys.getattr("stdout"))
                    .and_then(|stdout| stdout.call_method1("write", ("[sink]",)));
            }
        }

        Python::with_gil(|py| {
            let sink = Arc::new(Reentrant::default());
            with_string_io(py, sink.clone(), |_| {
                let code = CString::new("print('loop')").unwrap();
                py.run(code.as_c_str(), None, None)?;
                Ok(())
            })
            .unwrap();
            let events = sink.inner.events();
            let meaningful: Vec<&[u8]> = events
                .iter()
                .map(|event| event.payload.as_slice())
                .filter(|payload| !payload.is_empty() && *payload != b"\n")
                .collect();
            assert_eq!(meaningful.len(), 1);
            assert_eq!(meaningful[0], b"loop");
        });
    }
}
