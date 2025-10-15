use crate::runtime::io_capture::events::{IoOperation, IoStream, ProxyEvent, ProxySink};
use crate::runtime::io_capture::fd_mirror::{LedgerTicket, MirrorLedgers};
use pyo3::exceptions::PyStopIteration;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyAnyMethods, PyList, PyTuple};
use pyo3::IntoPyObject;
use std::sync::Arc;
use std::thread::{self, ThreadId};
use std::time::Instant;

fn current_thread_id() -> ThreadId {
    thread::current().id()
}

fn now() -> Instant {
    Instant::now()
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
// See test coverage in `install.rs`.
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

struct OutputProxy {
    original: PyObject,
    sink: Arc<dyn ProxySink>,
    stream: IoStream,
    ledgers: Option<MirrorLedgers>,
}

impl OutputProxy {
    fn new(
        original: PyObject,
        sink: Arc<dyn ProxySink>,
        stream: IoStream,
        ledgers: Option<MirrorLedgers>,
    ) -> Self {
        Self {
            original,
            sink,
            stream,
            ledgers,
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
            path_id: None,
            line: None,
            path: None,
        };
        self.sink.record(py, event);
    }

    fn begin_ledger_entry(&self, payload: &[u8]) -> Option<LedgerTicket> {
        if payload.is_empty() {
            return None;
        }
        self.ledgers
            .as_ref()
            .and_then(|ledgers| ledgers.begin_proxy_write(self.stream, payload))
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
        let mut ticket: Option<LedgerTicket> = None;
        if entered {
            if let Some(bytes) = payload.as_ref() {
                ticket = self.begin_ledger_entry(bytes);
            }
        }
        let result = self
            .original
            .call_method1(py, method, args)
            .map(|value| value.into());
        if entered {
            if let (Ok(_), Some(data)) = (&result, payload) {
                self.record(py, operation, data);
                if let Some(ticket) = ticket.take() {
                    ticket.commit();
                }
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
    pub fn new(
        original: PyObject,
        sink: Arc<dyn ProxySink>,
        ledgers: Option<MirrorLedgers>,
    ) -> Self {
        Self {
            inner: OutputProxy::new(original, sink, IoStream::Stdout, ledgers),
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
            .call_method1(py, "flush", (), Some(Vec::new()), IoOperation::Flush)
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
    pub fn new(
        original: PyObject,
        sink: Arc<dyn ProxySink>,
        ledgers: Option<MirrorLedgers>,
    ) -> Self {
        Self {
            inner: OutputProxy::new(original, sink, IoStream::Stderr, ledgers),
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
            .call_method1(py, "flush", (), Some(Vec::new()), IoOperation::Flush)
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
            path_id: None,
            line: None,
            path: None,
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
        self.original.bind(py).getattr(name).map(|obj| obj.unbind())
    }
}
