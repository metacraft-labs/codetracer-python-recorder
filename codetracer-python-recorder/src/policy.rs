//! Runtime configuration policy for the recorder.

use std::env;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::RwLock;

use once_cell::sync::OnceCell;
use recorder_errors::{usage, ErrorCode, RecorderError, RecorderResult};

/// Environment variable configuring how the recorder reacts to internal errors.
pub const ENV_ON_RECORDER_ERROR: &str = "CODETRACER_ON_RECORDER_ERROR";
/// Environment variable enforcing that a trace file must be produced.
pub const ENV_REQUIRE_TRACE: &str = "CODETRACER_REQUIRE_TRACE";
/// Environment variable toggling whether partial trace files are kept.
pub const ENV_KEEP_PARTIAL_TRACE: &str = "CODETRACER_KEEP_PARTIAL_TRACE";
/// Environment variable controlling log level for the recorder crate.
pub const ENV_LOG_LEVEL: &str = "CODETRACER_LOG_LEVEL";
/// Environment variable pointing to a log destination file.
pub const ENV_LOG_FILE: &str = "CODETRACER_LOG_FILE";
/// Environment variable enabling JSON error trailers on stderr.
pub const ENV_JSON_ERRORS: &str = "CODETRACER_JSON_ERRORS";

static POLICY: OnceCell<RwLock<RecorderPolicy>> = OnceCell::new();

fn policy_cell() -> &'static RwLock<RecorderPolicy> {
    POLICY.get_or_init(|| RwLock::new(RecorderPolicy::default()))
}

/// Behaviour when the recorder encounters an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnRecorderError {
    /// Propagate the error to callers; tracing stops with a non-zero exit.
    Abort,
    /// Disable tracing but allow the host process to continue running.
    Disable,
}

impl Default for OnRecorderError {
    fn default() -> Self {
        OnRecorderError::Abort
    }
}

#[derive(Debug)]
pub struct PolicyParseError(pub RecorderError);

impl FromStr for OnRecorderError {
    type Err = PolicyParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "abort" => Ok(OnRecorderError::Abort),
            "disable" => Ok(OnRecorderError::Disable),
            other => Err(PolicyParseError(usage!(
                ErrorCode::InvalidPolicyValue,
                "invalid on_recorder_error value '{}' (expected 'abort' or 'disable')",
                other
            ))),
        }
    }
}

/// Recorder-wide runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecorderPolicy {
    pub on_recorder_error: OnRecorderError,
    pub require_trace: bool,
    pub keep_partial_trace: bool,
    pub log_level: Option<String>,
    pub log_file: Option<PathBuf>,
    pub json_errors: bool,
}

impl Default for RecorderPolicy {
    fn default() -> Self {
        Self {
            on_recorder_error: OnRecorderError::Abort,
            require_trace: false,
            keep_partial_trace: false,
            log_level: None,
            log_file: None,
            json_errors: false,
        }
    }
}

impl RecorderPolicy {
    fn apply_update(&mut self, update: PolicyUpdate) {
        if let Some(on_err) = update.on_recorder_error {
            self.on_recorder_error = on_err;
        }
        if let Some(require_trace) = update.require_trace {
            self.require_trace = require_trace;
        }
        if let Some(keep_partial) = update.keep_partial_trace {
            self.keep_partial_trace = keep_partial;
        }
        if let Some(level) = update.log_level {
            self.log_level = match level.trim() {
                "" => None,
                other => Some(other.to_string()),
            };
        }
        if let Some(path) = update.log_file {
            self.log_file = match path {
                PolicyPath::Clear => None,
                PolicyPath::Value(pb) => Some(pb),
            };
        }
        if let Some(json_errors) = update.json_errors {
            self.json_errors = json_errors;
        }
    }
}

/// Internal helper representing path updates.
#[derive(Debug, Clone)]
enum PolicyPath {
    Clear,
    Value(PathBuf),
}

/// Mutation record for the policy.
#[derive(Debug, Default, Clone)]
struct PolicyUpdate {
    on_recorder_error: Option<OnRecorderError>,
    require_trace: Option<bool>,
    keep_partial_trace: Option<bool>,
    log_level: Option<String>,
    log_file: Option<PolicyPath>,
    json_errors: Option<bool>,
}

/// Snapshot the current policy.
pub fn policy_snapshot() -> RecorderPolicy {
    policy_cell().read().expect("policy lock poisoned").clone()
}

/// Apply the provided update to the global policy.
fn apply_policy_update(update: PolicyUpdate) {
    let mut guard = policy_cell().write().expect("policy lock poisoned");
    guard.apply_update(update);
}

/// Load policy overrides from environment variables.
pub fn configure_policy_from_env() -> RecorderResult<()> {
    let mut update = PolicyUpdate::default();

    if let Ok(value) = env::var(ENV_ON_RECORDER_ERROR) {
        let on_err = OnRecorderError::from_str(&value).map_err(|err| err.0)?;
        update.on_recorder_error = Some(on_err);
    }

    if let Ok(value) = env::var(ENV_REQUIRE_TRACE) {
        update.require_trace = Some(parse_bool(&value)?);
    }

    if let Ok(value) = env::var(ENV_KEEP_PARTIAL_TRACE) {
        update.keep_partial_trace = Some(parse_bool(&value)?);
    }

    if let Ok(value) = env::var(ENV_LOG_LEVEL) {
        update.log_level = Some(value);
    }

    if let Ok(value) = env::var(ENV_LOG_FILE) {
        let path = if value.trim().is_empty() {
            PolicyPath::Clear
        } else {
            PolicyPath::Value(PathBuf::from(value))
        };
        update.log_file = Some(path);
    }

    if let Ok(value) = env::var(ENV_JSON_ERRORS) {
        update.json_errors = Some(parse_bool(&value)?);
    }

    apply_policy_update(update);
    Ok(())
}

fn parse_bool(value: &str) -> RecorderResult<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "t" | "yes" | "y" => Ok(true),
        "0" | "false" | "f" | "no" | "n" => Ok(false),
        other => Err(usage!(
            ErrorCode::InvalidPolicyValue,
            "invalid boolean value '{}' (expected true/false)",
            other
        )),
    }
}

// === PyO3 helpers ===

use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::ffi;

#[pyfunction(name = "configure_policy")]
#[pyo3(signature = (on_recorder_error=None, require_trace=None, keep_partial_trace=None, log_level=None, log_file=None, json_errors=None))]
pub fn configure_policy_py(
    on_recorder_error: Option<&str>,
    require_trace: Option<bool>,
    keep_partial_trace: Option<bool>,
    log_level: Option<&str>,
    log_file: Option<&str>,
    json_errors: Option<bool>,
) -> PyResult<()> {
    let mut update = PolicyUpdate::default();

    if let Some(value) = on_recorder_error {
        match OnRecorderError::from_str(value) {
            Ok(parsed) => update.on_recorder_error = Some(parsed),
            Err(err) => return Err(ffi::map_recorder_error(err.0)),
        }
    }

    if let Some(value) = require_trace {
        update.require_trace = Some(value);
    }

    if let Some(value) = keep_partial_trace {
        update.keep_partial_trace = Some(value);
    }

    if let Some(value) = log_level {
        update.log_level = Some(value.to_string());
    }

    if let Some(value) = log_file {
        let path = if value.trim().is_empty() {
            PolicyPath::Clear
        } else {
            PolicyPath::Value(PathBuf::from(value))
        };
        update.log_file = Some(path);
    }

    if let Some(value) = json_errors {
        update.json_errors = Some(value);
    }

    apply_policy_update(update);
    Ok(())
}

#[pyfunction(name = "configure_policy_from_env")]
pub fn py_configure_policy_from_env() -> PyResult<()> {
    configure_policy_from_env().map_err(ffi::map_recorder_error)
}

#[pyfunction(name = "policy_snapshot")]
pub fn py_policy_snapshot(py: Python<'_>) -> PyResult<PyObject> {
    let snapshot = policy_snapshot();
    let dict = PyDict::new(py);
    dict.set_item(
        "on_recorder_error",
        match snapshot.on_recorder_error {
            OnRecorderError::Abort => "abort",
            OnRecorderError::Disable => "disable",
        },
    )?;
    dict.set_item("require_trace", snapshot.require_trace)?;
    dict.set_item("keep_partial_trace", snapshot.keep_partial_trace)?;
    if let Some(level) = snapshot.log_level.as_deref() {
        dict.set_item("log_level", level)?;
    } else {
        dict.set_item("log_level", py.None())?;
    }
    if let Some(path) = snapshot.log_file.as_ref() {
        dict.set_item("log_file", path.display().to_string())?;
    } else {
        dict.set_item("log_file", py.None())?;
    }
    dict.set_item("json_errors", snapshot.json_errors)?;
    Ok(dict.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn reset_policy() {
        let mut guard = super::policy_cell().write().expect("policy lock poisoned");
        *guard = RecorderPolicy::default();
    }

    #[test]
    fn default_policy_snapshot() {
        reset_policy();
        let snap = policy_snapshot();
        assert_eq!(snap.on_recorder_error, OnRecorderError::Abort);
        assert!(!snap.require_trace);
        assert!(!snap.keep_partial_trace);
        assert!(!snap.json_errors);
        assert!(snap.log_level.is_none());
        assert!(snap.log_file.is_none());
    }

    #[test]
    fn configure_policy_updates_fields() {
        reset_policy();
        let mut update = PolicyUpdate::default();
        update.on_recorder_error = Some(OnRecorderError::Disable);
        update.require_trace = Some(true);
        update.keep_partial_trace = Some(true);
        update.log_level = Some("debug".to_string());
        update.log_file = Some(PolicyPath::Value(PathBuf::from("/tmp/log.txt")));
        update.json_errors = Some(true);

        apply_policy_update(update);

        let snap = policy_snapshot();
        assert_eq!(snap.on_recorder_error, OnRecorderError::Disable);
        assert!(snap.require_trace);
        assert!(snap.keep_partial_trace);
        assert_eq!(snap.log_level.as_deref(), Some("debug"));
        assert_eq!(snap.log_file.as_deref(), Some(Path::new("/tmp/log.txt")));
        assert!(snap.json_errors);
        reset_policy();
    }

    #[test]
    fn configure_policy_from_env_parses_values() {
        reset_policy();
        let env_guard = env_lock();
        env::set_var(ENV_ON_RECORDER_ERROR, "disable");
        env::set_var(ENV_REQUIRE_TRACE, "true");
        env::set_var(ENV_KEEP_PARTIAL_TRACE, "1");
        env::set_var(ENV_LOG_LEVEL, "info");
        env::set_var(ENV_LOG_FILE, "/tmp/out.log");
        env::set_var(ENV_JSON_ERRORS, "yes");

        configure_policy_from_env().expect("configure from env");

        drop(env_guard);

        let snap = policy_snapshot();
        assert_eq!(snap.on_recorder_error, OnRecorderError::Disable);
        assert!(snap.require_trace);
        assert!(snap.keep_partial_trace);
        assert_eq!(snap.log_level.as_deref(), Some("info"));
        assert_eq!(snap.log_file.as_deref(), Some(Path::new("/tmp/out.log")));
        assert!(snap.json_errors);
        reset_policy();
    }

    fn env_lock() -> EnvGuard {
        EnvGuard
    }

    struct EnvGuard;

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for key in [
                ENV_ON_RECORDER_ERROR,
                ENV_REQUIRE_TRACE,
                ENV_KEEP_PARTIAL_TRACE,
                ENV_LOG_LEVEL,
                ENV_LOG_FILE,
                ENV_JSON_ERRORS,
            ] {
                env::remove_var(key);
            }
        }
    }
}
