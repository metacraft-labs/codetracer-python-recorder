//! Recorder-wide error façade for CodeTracer components.
//!
//! The crate defines a structured [`RecorderError`] type backed by
//! [`ErrorKind`] and [`ErrorCode`] classifications. Use the provided macros
//! (`usage!`, `enverr!`, `target!`, `bug!`, `ensure_usage!`, `ensure_env!`, etc.)
//! to author failures consistently across the workspace.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::error::Error as StdError;
use std::fmt;
use std::io;

/// Result alias used throughout the recorder workspace.
pub type RecorderResult<T> = Result<T, RecorderError>;

/// Key-value metadata associated with an error.
pub type ContextMap = BTreeMap<&'static str, String>;

/// High-level grouping of recorder failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ErrorKind {
    /// Caller provided invalid input or violated usage constraints.
    Usage,
    /// External environment or IO prevented the recorder from continuing.
    Environment,
    /// Target code being traced raised/behaved unexpectedly.
    Target,
    /// Internal bug or invariant violation inside the recorder.
    Internal,
}

/// Stable error codes used for analytics and tooling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ErrorCode {
    /// Fallback code used when no specific code applies yet.
    Unknown,
    /// Attempted to start tracing while another session runs.
    AlreadyTracing,
    /// Requested trace directory exists but is not a directory.
    TraceDirectoryConflict,
    /// Failed to create the trace directory due to IO errors.
    TraceDirectoryCreateFailed,
    /// User requested an unsupported trace format.
    UnsupportedFormat,
    /// Introspection of positional arguments failed.
    MissingPositionalArgument,
    /// Introspection of keyword arguments failed.
    MissingKeywordArgument,
    /// Failed to resolve frame locals or metadata.
    FrameIntrospectionFailed,
    /// Failed to resolve frame globals during introspection.
    GlobalsIntrospectionFailed,
    /// Attempted to install multiple tracers simultaneously.
    TracerInstallConflict,
    /// General IO failure propagated from lower layers.
    Io,
}

/// Canonical error type flowing through the recorder workspace.
#[derive(Debug)]
pub struct RecorderError {
    pub kind: ErrorKind,
    pub code: ErrorCode,
    pub message: Cow<'static, str>,
    pub context: ContextMap,
    source: Option<Box<dyn StdError + Send + Sync + 'static>>,
}

impl RecorderError {
    /// Create a new error with the provided classification and message.
    pub fn new(kind: ErrorKind, code: ErrorCode, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            kind,
            code,
            message: message.into(),
            context: ContextMap::new(),
            source: None,
        }
    }

    /// Attach a context key/value pair to the error.
    pub fn with_context(mut self, key: &'static str, value: impl Into<String>) -> Self {
        self.context.insert(key, value.into());
        self
    }

    /// Attach an underlying error source.
    pub fn with_source<E>(mut self, source: E) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        self.source = Some(Box::new(source));
        self
    }

    /// Update the error message while retaining classification and metadata.
    pub fn with_message(mut self, message: impl Into<Cow<'static, str>>) -> Self {
        self.message = message.into();
        self
    }

    /// Borrow the optional underlying source.
    pub fn source_ref(&self) -> Option<&(dyn StdError + Send + Sync + 'static)> {
        self.source.as_deref()
    }
}

impl fmt::Display for RecorderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl StdError for RecorderError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_deref()
            .map(|err| err as &(dyn StdError + 'static))
    }
}

impl From<io::Error> for RecorderError {
    fn from(err: io::Error) -> Self {
        RecorderError::new(ErrorKind::Environment, ErrorCode::Io, err.to_string()).with_source(err)
    }
}

/// Declare a recorder error using formatting syntax.
#[macro_export]
macro_rules! recorder_error {
    ($kind:expr, $code:expr, $msg:literal $(,)?) => {
        $crate::RecorderError::new($kind, $code, $msg)
    };
    ($kind:expr, $code:expr, $fmt:expr, $($arg:tt)+) => {
        $crate::RecorderError::new($kind, $code, ::std::borrow::Cow::Owned(format!($fmt, $($arg)+)))
    };
}

/// Convenience macro for usage errors.
#[macro_export]
macro_rules! usage {
    ($code:expr, $msg:literal $(,)?) => {
        $crate::recorder_error!($crate::ErrorKind::Usage, $code, $msg)
    };
    ($code:expr, $fmt:expr, $($arg:tt)+) => {
        $crate::recorder_error!($crate::ErrorKind::Usage, $code, $fmt, $($arg)+)
    };
}

/// Convenience macro for environment/IO errors.
#[macro_export]
macro_rules! enverr {
    ($code:expr, $msg:literal $(,)?) => {
        $crate::recorder_error!($crate::ErrorKind::Environment, $code, $msg)
    };
    ($code:expr, $fmt:expr, $($arg:tt)+) => {
        $crate::recorder_error!($crate::ErrorKind::Environment, $code, $fmt, $($arg)+)
    };
}

/// Convenience macro for target errors.
#[macro_export]
macro_rules! target {
    ($code:expr, $msg:literal $(,)?) => {
        $crate::recorder_error!($crate::ErrorKind::Target, $code, $msg)
    };
    ($code:expr, $fmt:expr, $($arg:tt)+) => {
        $crate::recorder_error!($crate::ErrorKind::Target, $code, $fmt, $($arg)+)
    };
}

/// Convenience macro for internal bugs/invariants.
#[macro_export]
macro_rules! bug {
    ($code:expr, $msg:literal $(,)?) => {
        $crate::recorder_error!($crate::ErrorKind::Internal, $code, $msg)
    };
    ($code:expr, $fmt:expr, $($arg:tt)+) => {
        $crate::recorder_error!($crate::ErrorKind::Internal, $code, $fmt, $($arg)+)
    };
}

/// Ensure a predicate holds, returning a usage error when it does not.
#[macro_export]
macro_rules! ensure_usage {
    ($cond:expr, $code:expr, $msg:literal $(,)?) => {
        if !$cond {
            return Err($crate::usage!($code, $msg));
        }
    };
    ($cond:expr, $code:expr, $fmt:expr, $($arg:tt)+) => {
        if !$cond {
            return Err($crate::usage!($code, $fmt, $($arg)+));
        }
    };
}

/// Ensure a predicate holds, returning an environment error when it does not.
#[macro_export]
macro_rules! ensure_env {
    ($cond:expr, $code:expr, $msg:literal $(,)?) => {
        if !$cond {
            return Err($crate::enverr!($code, $msg));
        }
    };
    ($cond:expr, $code:expr, $fmt:expr, $($arg:tt)+) => {
        if !$cond {
            return Err($crate::enverr!($code, $fmt, $($arg)+));
        }
    };
}

/// Ensure a predicate holds, returning an internal bug otherwise.
#[macro_export]
macro_rules! ensure_internal {
    ($cond:expr, $code:expr, $msg:literal $(,)?) => {
        if !$cond {
            return Err($crate::bug!($code, $msg));
        }
    };
    ($cond:expr, $code:expr, $fmt:expr, $($arg:tt)+) => {
        if !$cond {
            return Err($crate::bug!($code, $fmt, $($arg)+));
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_macro_builds_error() {
        let err = usage!(ErrorCode::UnsupportedFormat, "unsupported");
        assert_eq!(err.kind, ErrorKind::Usage);
        assert_eq!(err.code, ErrorCode::UnsupportedFormat);
        assert_eq!(err.message, "unsupported");
    }

    #[test]
    fn ensure_usage_fails() -> RecorderResult<()> {
        fn guarded(value: bool) -> RecorderResult<()> {
            ensure_usage!(value, ErrorCode::AlreadyTracing, "still tracing");
            Ok(())
        }

        let failure = guarded(false).expect_err("guard should fail");
        assert_eq!(failure.kind, ErrorKind::Usage);
        assert_eq!(failure.code, ErrorCode::AlreadyTracing);
        guarded(true)?;
        Ok(())
    }

    #[test]
    fn context_and_source_roundtrip() {
        let io_err = io::Error::new(io::ErrorKind::Other, "disk full");
        let err = RecorderError::from(io_err)
            .with_context("path", "/tmp/out")
            .with_message("failed to write trace");
        assert_eq!(err.kind, ErrorKind::Environment);
        assert_eq!(err.code, ErrorCode::Io);
        assert_eq!(err.context.get("path"), Some(&"/tmp/out".to_string()));
        assert_eq!(err.to_string(), "failed to write trace");
        assert!(err.source_ref().is_some());
    }

    #[test]
    fn ensure_env_reports_environment() {
        fn guarded(value: bool) -> RecorderResult<()> {
            ensure_env!(value, ErrorCode::TraceDirectoryCreateFailed, "io failure");
            Ok(())
        }

        let err = guarded(false).expect_err("should fail");
        assert_eq!(err.kind, ErrorKind::Environment);
        assert_eq!(err.code, ErrorCode::TraceDirectoryCreateFailed);
    }

    #[test]
    fn bug_macro_marks_internal() {
        let err = bug!(ErrorCode::FrameIntrospectionFailed, "panic avoided");
        assert_eq!(err.kind, ErrorKind::Internal);
        assert_eq!(err.code, ErrorCode::FrameIntrospectionFailed);
    }
}
