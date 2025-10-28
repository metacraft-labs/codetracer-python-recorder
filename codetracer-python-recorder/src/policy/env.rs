//! Environment variable parsing for recorder policy overrides.

use crate::policy::model::{apply_policy_update, OnRecorderError, PolicyPath, PolicyUpdate};
use recorder_errors::{usage, ErrorCode, RecorderResult};
use std::env;
use std::str::FromStr;

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
/// Environment variable toggling IO capture strategies.
pub const ENV_CAPTURE_IO: &str = "CODETRACER_CAPTURE_IO";
/// Environment variable toggling globals-based module name resolution.
pub const ENV_MODULE_NAME_FROM_GLOBALS: &str = "CODETRACER_MODULE_NAME_FROM_GLOBALS";
/// Environment variable toggling whether the recorder mirrors script exit codes.
pub const ENV_PROPAGATE_SCRIPT_EXIT: &str = "CODETRACER_PROPAGATE_SCRIPT_EXIT";

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
            PolicyPath::Value(value.into())
        };
        update.log_file = Some(path);
    }

    if let Ok(value) = env::var(ENV_JSON_ERRORS) {
        update.json_errors = Some(parse_bool(&value)?);
    }

    if let Ok(value) = env::var(ENV_CAPTURE_IO) {
        let (line_proxies, fd_fallback) = parse_capture_io(&value)?;
        update.io_capture_line_proxies = Some(line_proxies);
        update.io_capture_fd_fallback = Some(fd_fallback);
    }

    if let Ok(value) = env::var(ENV_MODULE_NAME_FROM_GLOBALS) {
        update.module_name_from_globals = Some(parse_bool(&value)?);
    }

    if let Ok(value) = env::var(ENV_PROPAGATE_SCRIPT_EXIT) {
        update.propagate_script_exit = Some(parse_bool(&value)?);
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

fn parse_capture_io(value: &str) -> RecorderResult<(bool, bool)> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        let default = crate::policy::model::IoCapturePolicy::default();
        return Ok((default.line_proxies, default.fd_fallback));
    }

    let lower = trimmed.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "0" | "off" | "false" | "disable" | "disabled" | "none"
    ) {
        return Ok((false, false));
    }
    if matches!(lower.as_str(), "1" | "on" | "true" | "enable" | "enabled") {
        return Ok((true, false));
    }

    let mut line_proxies = false;
    let mut fd_fallback = false;
    for token in lower.split(|c| matches!(c, ',' | '+')) {
        match token.trim() {
            "" => {}
            "proxies" | "proxy" => line_proxies = true,
            "fd" | "mirror" | "fallback" => {
                line_proxies = true;
                fd_fallback = true;
            }
            other => {
                return Err(usage!(
                    ErrorCode::InvalidPolicyValue,
                    "invalid CODETRACER_CAPTURE_IO value '{}'",
                    other
                ));
            }
        }
    }

    if !line_proxies && !fd_fallback {
        return Err(usage!(
            ErrorCode::InvalidPolicyValue,
            "CODETRACER_CAPTURE_IO must enable at least 'proxies' or 'fd'"
        ));
    }

    Ok((line_proxies, fd_fallback))
}

#[cfg(test)]
mod tests {
    #[cfg(test)]
    use super::*;
    use crate::policy::model::{policy_snapshot, reset_policy_for_tests};

    #[test]
    fn configure_policy_from_env_updates_fields() {
        let _guard = EnvGuard;
        reset_policy_for_tests();
        std::env::set_var(ENV_ON_RECORDER_ERROR, "disable");
        std::env::set_var(ENV_REQUIRE_TRACE, "true");
        std::env::set_var(ENV_KEEP_PARTIAL_TRACE, "1");
        std::env::set_var(ENV_LOG_LEVEL, "info");
        std::env::set_var(ENV_LOG_FILE, "/tmp/out.log");
        std::env::set_var(ENV_JSON_ERRORS, "yes");
        std::env::set_var(ENV_CAPTURE_IO, "proxies,fd");
        std::env::set_var(ENV_MODULE_NAME_FROM_GLOBALS, "true");
        std::env::set_var(ENV_PROPAGATE_SCRIPT_EXIT, "true");

        configure_policy_from_env().expect("configure from env");
        let snap = policy_snapshot();
        assert_eq!(snap.on_recorder_error, OnRecorderError::Disable);
        assert!(snap.require_trace);
        assert!(snap.keep_partial_trace);
        assert_eq!(snap.log_level.as_deref(), Some("info"));
        assert_eq!(
            snap.log_file.as_ref().map(|p| p.display().to_string()),
            Some("/tmp/out.log".to_string())
        );
        assert!(snap.json_errors);
        assert!(snap.io_capture.line_proxies);
        assert!(snap.io_capture.fd_fallback);
        assert!(snap.module_name_from_globals);
        assert!(snap.propagate_script_exit);
    }

    #[test]
    fn configure_policy_from_env_disables_module_name_from_globals() {
        let _guard = EnvGuard;
        reset_policy_for_tests();
        std::env::set_var(ENV_MODULE_NAME_FROM_GLOBALS, "false");
        std::env::set_var(ENV_PROPAGATE_SCRIPT_EXIT, "false");

        configure_policy_from_env().expect("configure from env");
        let snap = policy_snapshot();
        assert!(!snap.module_name_from_globals);
        assert!(!snap.propagate_script_exit);
    }

    #[test]
    fn parse_capture_io_handles_aliases() {
        assert_eq!(parse_capture_io("proxies+fd").unwrap(), (true, true));
        assert_eq!(parse_capture_io("proxies").unwrap(), (true, false));

        assert!(parse_capture_io("invalid-token").is_err());
    }

    #[test]
    fn parse_bool_rejects_invalid() {
        assert!(parse_bool("sometimes").is_err());
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
                ENV_PROPAGATE_SCRIPT_EXIT,
            ] {
                std::env::remove_var(key);
            }
        }
    }
}
