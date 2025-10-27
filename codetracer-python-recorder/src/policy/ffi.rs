//! PyO3 bindings exposing policy configuration to Python callers.

use super::env::configure_policy_from_env;
use super::model::{
    apply_policy_update, policy_snapshot, OnRecorderError, PolicyPath, PolicyUpdate,
};
use crate::ffi;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::path::PathBuf;
use std::str::FromStr;

#[pyfunction(name = "configure_policy")]
#[pyo3(signature = (on_recorder_error=None, require_trace=None, keep_partial_trace=None, log_level=None, log_file=None, json_errors=None, io_capture_line_proxies=None, io_capture_fd_fallback=None, module_name_from_globals=None))]
pub fn configure_policy_py(
    on_recorder_error: Option<&str>,
    require_trace: Option<bool>,
    keep_partial_trace: Option<bool>,
    log_level: Option<&str>,
    log_file: Option<&str>,
    json_errors: Option<bool>,
    io_capture_line_proxies: Option<bool>,
    io_capture_fd_fallback: Option<bool>,
    module_name_from_globals: Option<bool>,
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

    if let Some(value) = module_name_from_globals {
        update.module_name_from_globals = Some(value);
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
    dict.set_item(
        "module_name_from_globals",
        snapshot.module_name_from_globals,
    )?;

    let io_dict = PyDict::new(py);
    io_dict.set_item("line_proxies", snapshot.io_capture.line_proxies)?;
    io_dict.set_item("fd_fallback", snapshot.io_capture.fd_fallback)?;
    dict.set_item("io_capture", io_dict)?;
    Ok(dict.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::model::{policy_snapshot, reset_policy_for_tests};
    use pyo3::Python;

    #[test]
    fn configure_policy_py_updates_policy() {
        reset_policy_for_tests();
        configure_policy_py(
            Some("disable"),
            Some(true),
            Some(true),
            Some("debug"),
            Some("/tmp/log.txt"),
            Some(true),
            Some(true),
            Some(true),
            Some(true),
        )
        .expect("configure policy via PyO3 facade");

        let snap = policy_snapshot();
        assert_eq!(snap.on_recorder_error, OnRecorderError::Disable);
        assert!(snap.require_trace);
        assert!(snap.keep_partial_trace);
        assert_eq!(snap.log_level.as_deref(), Some("debug"));
        assert_eq!(
            snap.log_file
                .as_ref()
                .map(|p| p.display().to_string())
                .as_deref(),
            Some("/tmp/log.txt")
        );
        assert!(snap.json_errors);
        assert!(snap.io_capture.line_proxies);
        assert!(snap.io_capture.fd_fallback);
        assert!(snap.module_name_from_globals);
        reset_policy_for_tests();
    }

    #[test]
    fn configure_policy_py_rejects_invalid_on_recorder_error() {
        reset_policy_for_tests();
        let err = configure_policy_py(
            Some("unknown"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect_err("invalid variant should error");
        // Ensure the error maps through map_recorder_error by checking the display text.
        let message = Python::with_gil(|py| err.value(py).to_string());
        assert!(
            message.contains("invalid on_recorder_error value"),
            "unexpected error message: {message}"
        );
        reset_policy_for_tests();
    }

    #[test]
    fn py_configure_policy_from_env_propagates_error() {
        reset_policy_for_tests();
        let _guard = EnvGuard;
        std::env::set_var(super::super::env::ENV_REQUIRE_TRACE, "maybe");
        Python::with_gil(|py| {
            let err = py_configure_policy_from_env().expect_err("invalid env should error");
            let message = err.value(py).to_string();
            assert!(
                message.contains("invalid boolean value"),
                "unexpected error message: {message}"
            );
        });
        reset_policy_for_tests();
    }

    #[test]
    fn py_policy_snapshot_matches_model() {
        reset_policy_for_tests();
        configure_policy_py(
            Some("disable"),
            Some(true),
            Some(true),
            Some("info"),
            Some(""),
            Some(true),
            Some(false),
            Some(false),
            Some(false),
        )
        .expect("configure policy");

        Python::with_gil(|py| {
            let obj = py_policy_snapshot(py).expect("snapshot dict");
            let dict = obj.bind(py).downcast::<PyDict>().expect("dict");

            assert!(
                dict.contains("on_recorder_error")
                    .expect("check on_recorder_error key"),
                "expected on_recorder_error in snapshot"
            );
            assert!(
                dict.contains("io_capture").expect("check io_capture key"),
                "expected io_capture in snapshot"
            );
        });
        reset_policy_for_tests();
    }

    struct EnvGuard;

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for key in [
                super::super::env::ENV_ON_RECORDER_ERROR,
                super::super::env::ENV_REQUIRE_TRACE,
                super::super::env::ENV_KEEP_PARTIAL_TRACE,
                super::super::env::ENV_LOG_LEVEL,
                super::super::env::ENV_LOG_FILE,
                super::super::env::ENV_JSON_ERRORS,
                super::super::env::ENV_CAPTURE_IO,
                super::super::env::ENV_MODULE_NAME_FROM_GLOBALS,
            ] {
                std::env::remove_var(key);
            }
        }
    }
}
