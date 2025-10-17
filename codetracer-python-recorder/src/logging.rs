//! Diagnostics utilities: structured logging, metrics sinks, and error trailers.

mod logger;
mod metrics;
mod trailer;

pub use logger::{
    init_rust_logging_with_default, log_recorder_error, set_active_trace_id, with_error_code,
    with_error_code_opt,
};
pub use metrics::{
    install_metrics, record_detach, record_dropped_event, record_panic, RecorderMetrics,
};
pub use trailer::emit_error_trailer;

#[cfg(test)]
pub use metrics::test_support;
#[cfg(test)]
pub use trailer::set_error_trailer_writer_for_tests;

use crate::policy::RecorderPolicy;
use pyo3::types::PyAnyMethods;
use recorder_errors::ErrorCode;

pub fn apply_policy(policy: &RecorderPolicy) {
    logger::apply_logger_policy(policy);
    trailer::set_json_errors_enabled(policy.json_errors);
}

/// Attempt to read an `ErrorCode` attribute from a Python exception value.
pub fn error_code_from_pyerr(py: pyo3::Python<'_>, err: &pyo3::PyErr) -> Option<ErrorCode> {
    let value = err.value(py);
    let attr = value.getattr("code").ok()?;
    let code_str: String = attr.extract().ok()?;
    ErrorCode::parse(&code_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use once_cell::sync::OnceCell;
    use recorder_errors::{ErrorCode, ErrorKind, RecorderError};
    use serde_json::Value;
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};
    use tempfile::tempdir;

    fn ensure_logger() {
        init_rust_logging_with_default("codetracer_python_recorder=debug");
    }

    fn build_policy() -> crate::policy::RecorderPolicy {
        crate::policy::RecorderPolicy::default()
    }

    struct VecWriter {
        buf: Arc<Mutex<Vec<u8>>>,
    }

    impl VecWriter {
        fn new(buf: Arc<Mutex<Vec<u8>>>) -> Self {
            Self { buf }
        }
    }

    impl Write for VecWriter {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            let mut guard = self.buf.lock().expect("buffer lock");
            guard.extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn structured_log_records_run_and_error_code() {
        ensure_logger();
        let tmp = tempdir().expect("tempdir");
        let log_path = tmp.path().join("recorder.log");

        let mut policy = build_policy();
        policy.log_level = Some("debug".to_string());
        policy.log_file = Some(log_path.clone());
        apply_policy(&policy);

        with_error_code(ErrorCode::TraceMissing, || {
            log::error!(target: "codetracer_python_recorder::tests", "sample message");
        });

        log::logger().flush();

        let contents = std::fs::read_to_string(&log_path).expect("read log file");
        let line = contents.lines().last().expect("log line");
        let json: Value = serde_json::from_str(line).expect("valid json log");

        assert!(json.get("run_id").and_then(Value::as_str).is_some());
        assert_eq!(
            json.get("error_code").and_then(Value::as_str),
            Some("ERR_TRACE_MISSING")
        );
        assert_eq!(
            json.get("message").and_then(Value::as_str),
            Some("sample message")
        );

        apply_policy(&crate::policy::RecorderPolicy::default());
    }

    #[test]
    fn json_error_trailers_emit_payload() {
        ensure_logger();
        static BUFFER: OnceCell<Arc<Mutex<Vec<u8>>>> = OnceCell::new();
        let buf = BUFFER.get_or_init(|| {
            let buffer = Arc::new(Mutex::new(Vec::new()));
            let writer = VecWriter::new(buffer.clone());
            set_error_trailer_writer_for_tests(Box::new(writer));
            buffer
        });
        buf.lock().expect("buffer lock").clear();

        let mut policy = build_policy();
        policy.json_errors = true;
        apply_policy(&policy);

        let mut err = RecorderError::new(
            ErrorKind::Usage,
            ErrorCode::TraceMissing,
            "no trace produced",
        );
        err = err.with_context("path", "/tmp/trace".to_string());

        emit_error_trailer(&err);

        let data = buf.lock().expect("buffer lock").clone();
        let payload = String::from_utf8(data).expect("utf8");
        let line = payload.lines().last().expect("json line");
        let json: Value = serde_json::from_str(line).expect("valid trailer json");

        assert_eq!(
            json.get("error_code").and_then(Value::as_str),
            Some("ERR_TRACE_MISSING")
        );
        assert_eq!(
            json.get("message").and_then(Value::as_str),
            Some("no trace produced")
        );
        assert_eq!(
            json.get("context")
                .and_then(|ctx| ctx.get("path"))
                .and_then(Value::as_str),
            Some("/tmp/trace")
        );

        policy.json_errors = false;
        apply_policy(&policy);
    }

    #[test]
    fn metrics_sink_records_events() {
        let metrics = test_support::install();
        metrics.take();
        record_dropped_event("synthetic");
        record_detach("policy_disable", Some("ERR_TRACE_MISSING"));
        record_panic("ffi_guard");
        let events = metrics.take();
        assert!(events.contains(&test_support::MetricEvent::Dropped("synthetic")));
        assert!(events.contains(&test_support::MetricEvent::Detach(
            "policy_disable",
            Some("ERR_TRACE_MISSING".to_string())
        )));
        assert!(events.contains(&test_support::MetricEvent::Panic("ffi_guard")));
    }
}
