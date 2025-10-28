//! Policy data structures and in-memory management.

use once_cell::sync::OnceCell;
use recorder_errors::{usage, ErrorCode, RecorderError};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::RwLock;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IoCapturePolicy {
    pub line_proxies: bool,
    pub fd_fallback: bool,
}

impl Default for IoCapturePolicy {
    fn default() -> Self {
        Self {
            line_proxies: true,
            fd_fallback: false,
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
    pub io_capture: IoCapturePolicy,
    pub module_name_from_globals: bool,
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
            io_capture: IoCapturePolicy::default(),
            module_name_from_globals: true,
        }
    }
}

impl RecorderPolicy {
    pub(crate) fn apply_update(&mut self, update: PolicyUpdate) {
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
        if let Some(line_proxies) = update.io_capture_line_proxies {
            self.io_capture.line_proxies = line_proxies;
            if !self.io_capture.line_proxies {
                self.io_capture.fd_fallback = false;
            }
        }
        if let Some(fd_fallback) = update.io_capture_fd_fallback {
            // fd fallback requires proxies to be on.
            self.io_capture.fd_fallback = fd_fallback && self.io_capture.line_proxies;
        }
        if let Some(module_name_from_globals) = update.module_name_from_globals {
            self.module_name_from_globals = module_name_from_globals;
        }
    }
}

/// Internal helper representing path updates.
#[derive(Debug, Clone)]
pub(crate) enum PolicyPath {
    Clear,
    Value(PathBuf),
}

/// Mutation record for the policy.
#[derive(Debug, Default, Clone)]
pub(crate) struct PolicyUpdate {
    pub(crate) on_recorder_error: Option<OnRecorderError>,
    pub(crate) require_trace: Option<bool>,
    pub(crate) keep_partial_trace: Option<bool>,
    pub(crate) log_level: Option<String>,
    pub(crate) log_file: Option<PolicyPath>,
    pub(crate) json_errors: Option<bool>,
    pub(crate) io_capture_line_proxies: Option<bool>,
    pub(crate) io_capture_fd_fallback: Option<bool>,
    pub(crate) module_name_from_globals: Option<bool>,
}

/// Snapshot the current policy.
pub fn policy_snapshot() -> RecorderPolicy {
    policy_cell().read().expect("policy lock poisoned").clone()
}

/// Apply the provided update to the global policy and propagate logging changes.
pub(crate) fn apply_policy_update(update: PolicyUpdate) {
    let mut guard = policy_cell().write().expect("policy lock poisoned");
    guard.apply_update(update);
    crate::logging::apply_policy(&guard);
}

#[cfg(test)]
pub(crate) fn reset_policy_for_tests() {
    let mut guard = policy_cell().write().expect("policy lock poisoned");
    *guard = RecorderPolicy::default();
}
