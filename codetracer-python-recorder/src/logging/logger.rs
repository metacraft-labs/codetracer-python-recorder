use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::str::FromStr;
use std::sync::{Mutex, Once, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use log::{LevelFilter, Log, Metadata, Record};
use once_cell::sync::OnceCell;
use recorder_errors::{ErrorCode, RecorderError};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::policy::RecorderPolicy;

thread_local! {
    static ERROR_CODE_OVERRIDE: Cell<Option<ErrorCode>> = Cell::new(None);
}

static LOGGER_INSTANCE: OnceCell<&'static RecorderLogger> = OnceCell::new();
static INIT_LOGGER: Once = Once::new();

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

pub(crate) fn apply_logger_policy(policy: &RecorderPolicy) {
    if let Some(logger) = LOGGER_INSTANCE.get() {
        logger.apply_policy(policy);
    }
}

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

pub fn with_error_code_opt<F, R>(code: Option<ErrorCode>, op: F) -> R
where
    F: FnOnce() -> R,
{
    match code {
        Some(code) => with_error_code(code, op),
        None => with_error_code(ErrorCode::Unknown, op),
    }
}

pub fn set_active_trace_id(trace_id: Option<String>) {
    if let Some(logger) = LOGGER_INSTANCE.get() {
        let mut guard = logger.trace_id.write().expect("trace id lock");
        *guard = trace_id;
    }
}

pub fn log_recorder_error(label: &str, err: &RecorderError) {
    let message = build_error_text(err, Some(label));
    with_error_code(err.code, || {
        log::error!(target: "codetracer_python_recorder::errors", "{}", message);
    });
}

pub(crate) fn snapshot_run_and_trace() -> Option<(String, Option<String>)> {
    LOGGER_INSTANCE
        .get()
        .map(|logger| (logger.run_id.clone(), logger.snapshot_trace_id()))
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
                let mut stderr = io::stderr().lock();
                let _ = writeln!(
                    stderr,
                    "{{\"run_id\":\"{}\",\"message\":\"failed to encode log entry\"}}",
                    self.run_id
                );
            }
        }
    }

    fn snapshot_trace_id(&self) -> Option<String> {
        self.trace_id.read().expect("trace id lock").clone()
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
    fields: BTreeMap<String, Value>,
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
