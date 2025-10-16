use crate::runtime::io_capture::events::ProxySink;
use crate::runtime::io_capture::fd_mirror::MirrorLedgers;
use crate::runtime::io_capture::proxies::{LineAwareStderr, LineAwareStdin, LineAwareStdout};
use pyo3::prelude::*;
use std::sync::Arc;

#[cfg_attr(not(test), allow(dead_code))]
/// Controller that installs the proxies and restores the original streams.
pub struct IoStreamProxies {
    _stdout_proxy: Py<LineAwareStdout>,
    _stderr_proxy: Py<LineAwareStderr>,
    _stdin_proxy: Py<LineAwareStdin>,
    original_stdout: PyObject,
    original_stderr: PyObject,
    original_stdin: PyObject,
    installed: bool,
}

#[cfg_attr(not(test), allow(dead_code))]
impl IoStreamProxies {
    pub fn install(
        py: Python<'_>,
        sink: Arc<dyn ProxySink>,
        ledgers: Option<MirrorLedgers>,
    ) -> PyResult<Self> {
        let sys = py.import("sys")?;
        let stdout_original = sys.getattr("stdout")?.unbind();
        let stderr_original = sys.getattr("stderr")?.unbind();
        let stdin_original = sys.getattr("stdin")?.unbind();

        let stdout_proxy = Py::new(
            py,
            LineAwareStdout::new(stdout_original.clone_ref(py), sink.clone(), ledgers.clone()),
        )?;
        let stderr_proxy = Py::new(
            py,
            LineAwareStderr::new(stderr_original.clone_ref(py), sink.clone(), ledgers.clone()),
        )?;
        let stdin_proxy = Py::new(
            py,
            LineAwareStdin::new(stdin_original.clone_ref(py), sink.clone()),
        )?;

        sys.setattr("stdout", stdout_proxy.clone_ref(py))?;
        sys.setattr("stderr", stderr_proxy.clone_ref(py))?;
        sys.setattr("stdin", stdin_proxy.clone_ref(py))?;

        Ok(Self {
            _stdout_proxy: stdout_proxy,
            _stderr_proxy: stderr_proxy,
            _stdin_proxy: stdin_proxy,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::io_capture::events::{IoOperation, IoStream, ProxyEvent};
    use pyo3::Python;
    use std::ffi::CString;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingSink {
        events: Mutex<Vec<ProxyEvent>>,
    }

    impl RecordingSink {
        fn new() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
            }
        }

        fn events(&self) -> Vec<ProxyEvent> {
            self.events.lock().expect("lock poisoned").clone()
        }
    }

    impl ProxySink for RecordingSink {
        fn record(&self, _py: Python<'_>, event: ProxyEvent) {
            self.events.lock().expect("lock poisoned").push(event);
        }
    }

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

        let mut proxies = IoStreamProxies::install(py, sink, None)?;
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
            let payloads: Vec<&[u8]> = events
                .iter()
                .map(|event| event.payload.as_slice())
                .filter(|payload| !payload.is_empty() && *payload != b"\n")
                .collect();
            assert_eq!(payloads.len(), 1);
            assert_eq!(payloads[0], b"loop");
        });
    }
}
