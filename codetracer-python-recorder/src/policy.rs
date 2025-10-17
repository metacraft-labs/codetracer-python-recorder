//! Runtime configuration policy for the recorder.

mod env;
mod model;

#[allow(unused_imports)]
pub use env::{
    configure_policy_from_env, ENV_CAPTURE_IO, ENV_JSON_ERRORS, ENV_KEEP_PARTIAL_TRACE,
    ENV_LOG_FILE, ENV_LOG_LEVEL, ENV_ON_RECORDER_ERROR, ENV_REQUIRE_TRACE,
};
use model::{apply_policy_update, PolicyPath, PolicyUpdate};
#[allow(unused_imports)]
pub use model::{policy_snapshot, IoCapturePolicy, OnRecorderError, RecorderPolicy};
#[allow(unused_imports)]
pub use model::PolicyParseError;

use std::path::PathBuf;
use std::str::FromStr;

// === PyO3 helpers ===

use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::ffi;

#[pyfunction(name = "configure_policy")]
#[pyo3(signature = (on_recorder_error=None, require_trace=None, keep_partial_trace=None, log_level=None, log_file=None, json_errors=None, io_capture_line_proxies=None, io_capture_fd_fallback=None))]
pub fn configure_policy_py(
    on_recorder_error: Option<&str>,
    require_trace: Option<bool>,
    keep_partial_trace: Option<bool>,
    log_level: Option<&str>,
    log_file: Option<&str>,
    json_errors: Option<bool>,
    io_capture_line_proxies: Option<bool>,
    io_capture_fd_fallback: Option<bool>,
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

    if let Some(value) = io_capture_line_proxies {
        update.io_capture_line_proxies = Some(value);
    }

    if let Some(value) = io_capture_fd_fallback {
        update.io_capture_fd_fallback = Some(value);
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

    let io_dict = PyDict::new(py);
    io_dict.set_item("line_proxies", snapshot.io_capture.line_proxies)?;
    io_dict.set_item("fd_fallback", snapshot.io_capture.fd_fallback)?;
    dict.set_item("io_capture", io_dict)?;
    Ok(dict.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use recorder_errors::ErrorCode;
    use std::path::Path;

    fn reset_policy() {
        super::model::reset_policy_for_tests();
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
        assert!(snap.io_capture.line_proxies);
        assert!(!snap.io_capture.fd_fallback);
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
        update.io_capture_line_proxies = Some(true);
        update.io_capture_fd_fallback = Some(true);

        apply_policy_update(update);

        let snap = policy_snapshot();
        assert_eq!(snap.on_recorder_error, OnRecorderError::Disable);
        assert!(snap.require_trace);
        assert!(snap.keep_partial_trace);
        assert_eq!(snap.log_level.as_deref(), Some("debug"));
        assert_eq!(snap.log_file.as_deref(), Some(Path::new("/tmp/log.txt")));
        assert!(snap.json_errors);
        assert!(snap.io_capture.line_proxies);
        assert!(snap.io_capture.fd_fallback);
        reset_policy();
    }

    #[test]
    fn configure_policy_from_env_parses_values() {
        reset_policy();
        let env_guard = env_lock();
        std::env::set_var(ENV_ON_RECORDER_ERROR, "disable");
        std::env::set_var(ENV_REQUIRE_TRACE, "true");
        std::env::set_var(ENV_KEEP_PARTIAL_TRACE, "1");
        std::env::set_var(ENV_LOG_LEVEL, "info");
        std::env::set_var(ENV_LOG_FILE, "/tmp/out.log");
        std::env::set_var(ENV_JSON_ERRORS, "yes");
        std::env::set_var(ENV_CAPTURE_IO, "proxies,fd");

        configure_policy_from_env().expect("configure from env");

        drop(env_guard);

        let snap = policy_snapshot();
        assert_eq!(snap.on_recorder_error, OnRecorderError::Disable);
        assert!(snap.require_trace);
        assert!(snap.keep_partial_trace);
        assert_eq!(snap.log_level.as_deref(), Some("info"));
        assert_eq!(snap.log_file.as_deref(), Some(Path::new("/tmp/out.log")));
        assert!(snap.json_errors);
        assert!(snap.io_capture.line_proxies);
        assert!(snap.io_capture.fd_fallback);
        reset_policy();
    }

    #[test]
    fn configure_policy_from_env_accepts_plus_separator() {
        reset_policy();
        let env_guard = env_lock();
        std::env::set_var(ENV_CAPTURE_IO, "proxies+fd");

        configure_policy_from_env().expect("configure from env with plus separator");

        drop(env_guard);

        let snap = policy_snapshot();
        assert!(snap.io_capture.line_proxies);
        assert!(snap.io_capture.fd_fallback);
        reset_policy();
    }

    #[test]
    fn configure_policy_from_env_rejects_invalid_boolean() {
        reset_policy();
        let env_guard = env_lock();
        std::env::set_var(ENV_REQUIRE_TRACE, "sometimes");

        let err = configure_policy_from_env().expect_err("invalid bool should error");
        assert_eq!(err.code, ErrorCode::InvalidPolicyValue);

        drop(env_guard);
        reset_policy();
    }

    #[test]
    fn configure_policy_from_env_rejects_invalid_capture_io() {
        reset_policy();
        let env_guard = env_lock();
        std::env::set_var(ENV_CAPTURE_IO, "invalid-token");

        let err = configure_policy_from_env().expect_err("invalid capture io should error");
        assert_eq!(err.code, ErrorCode::InvalidPolicyValue);

        drop(env_guard);
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
                ENV_CAPTURE_IO,
            ] {
                std::env::remove_var(key);
        }
    }
}
}
