//! Runtime configuration policy for the recorder.

mod env;
mod ffi;
mod model;

#[allow(unused_imports)]
pub use env::{
    configure_policy_from_env, ENV_CAPTURE_IO, ENV_JSON_ERRORS, ENV_KEEP_PARTIAL_TRACE,
    ENV_LOG_FILE, ENV_LOG_LEVEL, ENV_MODULE_NAME_FROM_GLOBALS, ENV_ON_RECORDER_ERROR,
    ENV_REQUIRE_TRACE,
};
#[allow(unused_imports)]
pub use ffi::{configure_policy_py, py_configure_policy_from_env, py_policy_snapshot};
#[allow(unused_imports)]
pub use model::PolicyParseError;
#[allow(unused_imports)]
pub use model::{policy_snapshot, IoCapturePolicy, OnRecorderError, RecorderPolicy};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::model::{
        apply_policy_update, reset_policy_for_tests, PolicyPath, PolicyUpdate,
    };
    use recorder_errors::ErrorCode;
    use std::path::{Path, PathBuf};

    fn reset_policy() {
        reset_policy_for_tests();
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
        assert!(snap.module_name_from_globals);
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
        update.module_name_from_globals = Some(true);

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
        assert!(snap.module_name_from_globals);
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
        std::env::set_var(ENV_MODULE_NAME_FROM_GLOBALS, "true");

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
        assert!(snap.module_name_from_globals);
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
                ENV_MODULE_NAME_FROM_GLOBALS,
            ] {
                std::env::remove_var(key);
            }
        }
    }
}
