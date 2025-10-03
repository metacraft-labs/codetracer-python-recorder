//! Shared helpers for translating `RecorderError` into Python exceptions.

use recorder_errors::RecorderResult;

/// Convenient alias for recorder results used across the Rust modules.
pub type Result<T> = RecorderResult<T>;
