//! Diagnostics utilities: structured logging, metrics sinks, and error trailers.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Once, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use log::{LevelFilter, Log, Metadata, Record};
use once_cell::sync::OnceCell;
use pyo3::prelude::*;
use recorder_errors::{ErrorCode, RecorderError};
use serde::Serialize;
use uuid::Uuid;

use crate::policy::RecorderPolicy;

thread_local! {
    static ERROR_CODE_OVERRIDE: Cell<Option<ErrorCode>> = Cell::new(None);
}

static LOGGER_INSTANCE: OnceCell<&'static RecorderLogger> = OnceCell::new();
static INIT_LOGGER: Once = Once::new();
static JSON_ERRORS_ENABLED: AtomicBool = AtomicBool::new(false);
static ERROR_TRAILER_WRITER: OnceCell<Mutex<Box<dyn Write + Send>>> = OnceCell::new();
static METRICS_SINK: OnceCell<Box<dyn RecorderMetrics>> = OnceCell::new();

/// Structured logging initialisation applied on module import and during tracing start.
///
/// The first caller installs a process-wide logger that emits JSON records containing
/// `run_id`, `trace_id`, and optional `error_code` fields. Subsequent calls are no-ops.
pub fn init_rust_logging_with_default(default_filter: &str) {
    INIT_LOGGER.call_once(|| {
        let default_spec = FilterSpec::parse(default_filter, LevelFilter::Warn)
            .unwrap_or_else(|_| FilterSpec::new(LevelFilter::Warn));

        let initial_spec = std::env::var("RUST_LOG")
            .ok()
            .and_then(|spec| FilterSpec::parse(&spec, default_spec.global).ok())
            .unwrap_or_else(|| default_spec.clone());

        let logger = RecorderLogger::new(default_spec, initial_spec);
        let leaked: &'static RecorderLogger = Box::leak(Box::new(logger));
        log::set_logger(leaked).expect("recorder logger already initialised");
        log::set_max_level(leaked.filter.read().expect("filter lock").max_level());
        let _ = LOGGER_INSTANCE.set(leaked);
    });
}

/// Apply the current policy to logging and diagnostics outputs.
pub fn apply_policy(policy: &RecorderPolicy) {
    if let Some(logger) = LOGGER_INSTANCE.get() {
        logger.apply_policy(policy);
    }
    JSON_ERRORS_ENABLED.store(policy.json_errors, Ordering::SeqCst);
}

/// Scope a log emission with an explicit `ErrorCode` so the structured logger can attach it.
pub fn with_error_code<F, R>(code: ErrorCode, op: F) -> R
where
    F: FnOnce() -> R,
{
    ERROR_CODE_OVERRIDE.with(|cell| {
        let previous = cell.replace(Some(code));
        let result = op();
        cell.set(previous);
        result
    })
}

/// Scope a log emission with an optional `ErrorCode` (falls back to `ERR_UNKNOWN`).
pub fn with_error_code_opt<F, R>(code: Option<ErrorCode>, op: F) -> R
where
    F: FnOnce() -> R,
{
    match code {
        Some(code) => with_error_code(code, op),
        None => with_error_code(ErrorCode::Unknown, op),
    }
}

/// Update the active trace identifier associated with subsequent log records.
pub fn set_active_trace_id(trace_id: Option<String>) {
    if let Some(logger) = LOGGER_INSTANCE.get() {
        let mut guard = logger.trace_id.write().expect("trace id lock");
        *guard = trace_id;
    }
}

/// Log a structured representation of `err` for observability pipelines.
pub fn log_recorder_error(label: &str, err: &RecorderError) {
    let message = build_error_text(err, Some(label));
    with_error_code(err.code, || {
        log::error!(target: "codetracer_python_recorder::errors", "{}", message);
    });
}

/// Emit a JSON error trailer on stderr when the policy requests it.
pub fn emit_error_trailer(err: &RecorderError) {
    if !JSON_ERRORS_ENABLED.load(Ordering::SeqCst) {
        return;
    }

    let Some(logger) = LOGGER_INSTANCE.get() else {
        return;
    };

    let trace_id = logger.trace_id.read().expect("trace id lock").clone();

    let mut context = serde_json::Map::new();
    for (key, value) in &err.context {
        context.insert((*key).to_string(), serde_json::Value::String(value.clone()));
    }

    let payload = serde_json::json!({
        "run_id": logger.run_id,
        "trace_id": trace_id,
        "error_code": err.code.as_str(),
        "error_kind": format!("{:?}", err.kind),
        "message": err.message(),
        "context": context,
    });

    if let Ok(mut bytes) = serde_json::to_vec(&payload) {
        bytes.push(b'\n');
        if let Some(writer) = ERROR_TRAILER_WRITER.get() {
            let mut guard = writer.lock().expect("error trailer writer lock");
            let _ = guard.write_all(&bytes);
            let _ = guard.flush();
        } else {
            let mut stderr = io::stderr().lock();
            let _ = stderr.write_all(&bytes);
            let _ = stderr.flush();
        }
    }
}

/// Metrics interface allowing pluggable sinks (default: no-op).
pub trait RecorderMetrics: Send + Sync {
    /// Record that an event stream was dropped for the provided reason.
    fn record_dropped_event(&self, _reason: &'static str) {}
    /// Record that tracing detached, optionally linked to an error code.
    fn record_detach(&self, _reason: &'static str, _error_code: Option<&str>) {}
    /// Record that a panic was caught and converted into an error.
    fn record_panic(&self, _label: &'static str) {}
}

struct NoopMetrics;

impl RecorderMetrics for NoopMetrics {}

fn metrics_sink() -> &'static dyn RecorderMetrics {
    METRICS_SINK
        .get_or_init(|| Box::new(NoopMetrics) as Box<dyn RecorderMetrics>)
        .as_ref()
}

/// Install a custom metrics sink. Intended for embedding or tests.
#[cfg_attr(not(test), allow(dead_code))]
pub fn install_metrics(metrics: Box<dyn RecorderMetrics>) -> Result<(), Box<dyn RecorderMetrics>> {
    METRICS_SINK.set(metrics)
}

/// Record that we abandoned a monitoring location (e.g., synthetic filename).
pub fn record_dropped_event(reason: &'static str) {
    metrics_sink().record_dropped_event(reason);
}

/// Record that we detached per-policy or due to unrecoverable failure.
pub fn record_detach(reason: &'static str, error_code: Option<&str>) {
    metrics_sink().record_detach(reason, error_code);
}

/// Record that we caught a panic at the FFI boundary.
pub fn record_panic(label: &'static str) {
    metrics_sink().record_panic(label);
}

/// Attempt to read an `ErrorCode` attribute from a Python exception value.
pub fn error_code_from_pyerr(py: pyo3::Python<'_>, err: &pyo3::PyErr) -> Option<ErrorCode> {
    let value = err.value(py);
    let attr = value.getattr("code").ok()?;
    let code_str: String = attr.extract().ok()?;
    ErrorCode::parse(&code_str)
}

/// Provide a helper for tests to override the error trailer destination.
#[cfg(test)]
pub fn set_error_trailer_writer_for_tests(writer: Box<dyn Write + Send>) {
    let _ = ERROR_TRAILER_WRITER.set(Mutex::new(writer));
}

struct RecorderLogger {
    run_id: String,
    trace_id: RwLock<Option<String>>,
    default_filter: FilterSpec,
    filter: RwLock<FilterSpec>,
    writer: Mutex<Destination>,
}

impl RecorderLogger {
    fn new(default_filter: FilterSpec, initial: FilterSpec) -> Self {
        Self {
            run_id: Uuid::new_v4().to_string(),
            trace_id: RwLock::new(None),
            writer: Mutex::new(Destination::Stderr),
            filter: RwLock::new(initial),
            default_filter,
        }
    }

    fn apply_policy(&self, policy: &RecorderPolicy) {
        let new_filter = match policy.log_level.as_deref() {
            Some(spec) if !spec.trim().is_empty() => {
                match FilterSpec::parse(spec, self.default_filter.global) {
                    Ok(parsed) => parsed,
                    Err(_) => {
                        with_error_code(ErrorCode::InvalidPolicyValue, || {
                            log::warn!(
                                target: "codetracer_python_recorder::logging",
                                "invalid log level filter '{}'; reverting to default",
                                spec
                            );
                        });
                        self.default_filter.clone()
                    }
                }
            }
            _ => self.default_filter.clone(),
        };

        {
            let mut guard = self.filter.write().expect("filter lock");
            *guard = new_filter.clone();
        }
        log::set_max_level(new_filter.max_level());

        match policy.log_file.as_ref() {
            Some(path) => match open_log_file(path) {
                Ok(file) => {
                    *self.writer.lock().expect("writer lock") = Destination::File(file);
                }
                Err(err) => {
                    with_error_code(ErrorCode::Io, || {
                        log::warn!(
                            target: "codetracer_python_recorder::logging",
                            "failed to open log file '{}': {}",
                            path.display(),
                            err
                        );
                    });
                    *self.writer.lock().expect("writer lock") = Destination::Stderr;
                }
            },
            None => {
                *self.writer.lock().expect("writer lock") = Destination::Stderr;
            }
        }
    }

    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        self.filter.read().expect("filter lock").allows(metadata)
    }

    fn write_entry(&self, entry: &LogEntry<'_>) {
        match serde_json::to_vec(entry) {
            Ok(mut bytes) => {
                bytes.push(b'\n');
                if let Err(err) = self.writer.lock().expect("writer lock").write_all(&bytes) {
                    let mut stderr = io::stderr().lock();
                    let _ = stderr.write_all(&bytes);
                    let _ = writeln!(
                        stderr,
                        "{{\"run_id\":\"{}\",\"message\":\"logger write failure: {}\"}}",
                        self.run_id, err
                    );
                }
            }
            Err(_) => {
                // Fallback to plain message if serialization fails
                let mut stderr = io::stderr().lock();
                let _ = writeln!(
                    stderr,
                    "{{\"run_id\":\"{}\",\"message\":\"failed to encode log entry\"}}",
                    self.run_id
                );
            }
        }
    }
}

impl Log for RecorderLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        self.enabled(metadata)
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let thread_code = ERROR_CODE_OVERRIDE.with(|cell| cell.get());
        let error_code = thread_code.map(|code| code.as_str().to_string());
        let mut fields = BTreeMap::new();
        if let Some(code) = error_code.as_ref() {
            fields.insert(
                "error_code".to_string(),
                serde_json::Value::String(code.clone()),
            );
        }

        let trace_id = self.trace_id.read().expect("trace id lock").clone();

        let entry = LogEntry {
            ts_micros: current_timestamp_micros(),
            level: record.level().as_str(),
            target: record.target(),
            run_id: &self.run_id,
            trace_id: trace_id.as_deref(),
            message: record.args().to_string(),
            error_code,
            module_path: record.module_path(),
            file: record.file(),
            line: record.line(),
            fields,
        };

        self.write_entry(&entry);
    }

    fn flush(&self) {
        let _ = self.writer.lock().expect("writer lock").flush();
    }
}

#[derive(Clone)]
struct FilterSpec {
    global: LevelFilter,
    targets: Vec<(String, LevelFilter)>,
}

impl FilterSpec {
    fn new(global: LevelFilter) -> Self {
        Self {
            global,
            targets: Vec::new(),
        }
    }

    fn parse(spec: &str, default_global: LevelFilter) -> Result<Self, ()> {
        let mut filter = Self::new(default_global);
        for part in spec.split(',') {
            let trimmed = part.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some((target, level)) = trimmed.split_once('=') {
                let lvl = LevelFilter::from_str(level.trim()).map_err(|_| ())?;
                filter.targets.push((target.trim().to_string(), lvl));
            } else {
                filter.global = LevelFilter::from_str(trimmed).map_err(|_| ())?;
            }
        }
        Ok(filter)
    }

    fn allows(&self, metadata: &Metadata<'_>) -> bool {
        let mut allowed = self.global;
        let mut matched_len = 0usize;
        let target = metadata.target();
        for (pattern, level) in &self.targets {
            if target == pattern
                || target.starts_with(pattern) && target.chars().nth(pattern.len()) == Some(':')
            {
                if pattern.len() > matched_len {
                    matched_len = pattern.len();
                    allowed = *level;
                }
            }
        }
        allowed >= metadata.level().to_level_filter()
    }

    fn max_level(&self) -> LevelFilter {
        self.targets
            .iter()
            .fold(self.global, |acc, (_, lvl)| acc.max(*lvl))
    }
}

#[derive(Serialize)]
struct LogEntry<'a> {
    ts_micros: i128,
    level: &'a str,
    target: &'a str,
    run_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    trace_id: Option<&'a str>,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    module_path: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<u32>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    fields: BTreeMap<String, serde_json::Value>,
}

fn current_timestamp_micros() -> i128 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => {
            let secs = duration.as_secs() as i128;
            let micros = duration.subsec_micros() as i128;
            secs * 1_000_000 + micros
        }
        Err(_) => 0,
    }
}

enum Destination {
    Stderr,
    File(File),
}

impl Destination {
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        match self {
            Destination::Stderr => {
                let mut stderr = io::stderr().lock();
                stderr.write_all(bytes)?;
                stderr.flush()
            }
            Destination::File(file) => {
                file.write_all(bytes)?;
                file.flush()
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Destination::Stderr => io::stderr().lock().flush(),
            Destination::File(file) => file.flush(),
        }
    }
}

fn open_log_file(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

fn build_error_text(err: &RecorderError, label: Option<&str>) -> String {
    let mut text = String::new();
    if let Some(label) = label {
        text.push_str(label);
        text.push_str(": ");
    }
    text.push_str(err.message());
    if !err.context.is_empty() {
        text.push_str(" (");
        let mut first = true;
        for (key, value) in &err.context {
            if !first {
                text.push_str(", ");
            }
            first = false;
            text.push_str(key);
            text.push('=');
            text.push_str(value);
        }
        text.push(')');
    }
    text
}

#[cfg(test)]
pub mod test_support {
    use super::*;
    use once_cell::sync::OnceCell;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    pub struct CapturingMetrics {
        events: Arc<Mutex<Vec<MetricEvent>>>,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum MetricEvent {
        Dropped(&'static str),
        Detach(&'static str, Option<String>),
        Panic(&'static str),
    }

    impl CapturingMetrics {
        pub fn take(&self) -> Vec<MetricEvent> {
            let mut guard = self.events.lock().expect("metrics events lock");
            let events = guard.clone();
            guard.clear();
            events
        }
    }

    impl RecorderMetrics for CapturingMetrics {
        fn record_dropped_event(&self, reason: &'static str) {
            self.events
                .lock()
                .expect("metrics events lock")
                .push(MetricEvent::Dropped(reason));
        }

        fn record_detach(&self, reason: &'static str, error_code: Option<&str>) {
            self.events
                .lock()
                .expect("metrics events lock")
                .push(MetricEvent::Detach(
                    reason,
                    error_code.map(|s| s.to_string()),
                ));
        }

        fn record_panic(&self, label: &'static str) {
            self.events
                .lock()
                .expect("metrics events lock")
                .push(MetricEvent::Panic(label));
        }
    }

    static CAPTURING: OnceCell<CapturingMetrics> = OnceCell::new();

    pub fn install() -> &'static CapturingMetrics {
        CAPTURING.get_or_init(|| {
            let metrics = CapturingMetrics::default();
            let _ = super::install_metrics(Box::new(metrics.clone()));
            metrics
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::RecorderPolicy;
    use once_cell::sync::OnceCell;
    use recorder_errors::{ErrorCode, ErrorKind};
    use serde_json::Value;
    use std::sync::{Arc, Mutex};
    use tempfile::tempdir;

    fn ensure_logger() {
        init_rust_logging_with_default("codetracer_python_recorder=debug");
    }

    fn build_policy() -> RecorderPolicy {
        RecorderPolicy::default()
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

        apply_policy(&RecorderPolicy::default());
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
