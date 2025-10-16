use crate::runtime::io_capture::events::{IoOperation, IoStream, ProxyEvent, ProxySink};
use crate::runtime::io_capture::fd_mirror::{LedgerTicket, MirrorLedgers};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyAnyMethods, PyList, PyTuple};
use pyo3::IntoPyObject;
use std::sync::Arc;
use std::thread::{self, ThreadId};
use std::time::Instant;

pub(crate) fn current_thread_id() -> ThreadId {
    thread::current().id()
}

pub(crate) fn now() -> Instant {
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

pub(crate) fn buffer_snapshot(buffer: &Bound<'_, PyAny>) -> Option<Vec<u8>> {
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

pub(crate) fn enter_reentrancy_guard() -> bool {
    IN_PROXY_CALLBACK.with(|flag| {
        if flag.get() {
            false
        } else {
            flag.set(true);
            true
        }
    })
}

pub(crate) fn exit_reentrancy_guard(entered: bool) {
    if entered {
        IN_PROXY_CALLBACK.with(|flag| flag.set(false));
    }
}

#[allow(dead_code)]
pub(crate) struct OutputProxy {
    pub(crate) original: PyObject,
    sink: Arc<dyn ProxySink>,
    stream: IoStream,
    ledgers: Option<MirrorLedgers>,
}

impl OutputProxy {
    pub(crate) fn new(
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

    fn call_method_with_payload<'py, A>(
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

    pub(crate) fn passthrough<'py, A>(
        &self,
        py: Python<'py>,
        method: &str,
        args: A,
    ) -> PyResult<Py<PyAny>>
    where
        A: IntoPyObject<'py, Target = PyTuple>,
    {
        self.original
            .call_method1(py, method, args)
            .map(|value| value.into())
    }

    pub(crate) fn write(&self, py: Python<'_>, text: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let captured = text.extract::<String>()?.into_bytes();
        let args = (text.clone().unbind(),);
        self.call_method_with_payload(py, "write", args, Some(captured), IoOperation::Write)
    }

    pub(crate) fn writelines(
        &self,
        py: Python<'_>,
        lines: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let (captured, replay) = build_iterator_list(lines)?;
        let payload = captured.join("").into_bytes();
        self.call_method_with_payload(
            py,
            "writelines",
            (replay.clone_ref(py),),
            Some(payload),
            IoOperation::Writelines,
        )
    }

    pub(crate) fn flush(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.call_method_with_payload(py, "flush", (), Some(Vec::new()), IoOperation::Flush)
    }

    pub(crate) fn fileno(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.passthrough(py, "fileno", ())
    }

    pub(crate) fn isatty(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.passthrough(py, "isatty", ())
    }

    pub(crate) fn close(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.passthrough(py, "close", ())
    }

    pub(crate) fn getattr(&self, py: Python<'_>, name: &str) -> PyResult<PyObject> {
        self.original.bind(py).getattr(name).map(|obj| obj.unbind())
    }
}
