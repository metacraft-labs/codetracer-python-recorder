//! Trace-filter integration for the Python recorder.
//!
//! As of TF-M6 the cross-language filter implementation (selector grammar,
//! TOML schema, classifier algorithm, composition rules) lives in the
//! shared `codetracer_trace_filter` crate.  This module:
//!
//! 1. Re-exports the crate's public types under the historical module
//!    layout (`config`, `engine`, `selector`, `model`) so existing call
//!    sites compile without churn.
//! 2. Wraps the pure `Classifier` in a Python-aware `TraceFilterEngine`
//!    that maps `PyCode` objects into `ScopeQuery` values and caches the
//!    resulting `ScopeResolution` in CPython's native `co_extra` slot
//!    (spec § 6 — eliminates the last hash-lookup on the trace-emission
//!    hot path).
//! 3. Adapts the crate's `FilterError` to the recorder's `RecorderError`
//!    facade so callers continue to see `recorder_errors::RecorderResult`.

pub mod engine;

/// Re-export of the shared crate's selector grammar.  Existing call sites
/// reference `crate::trace_filter::selector::{Selector, SelectorKind, MatchType}`.
pub mod selector {
    pub use codetracer_trace_filter::selector::{MatchType, Selector, SelectorKind};
}

/// Re-export of the shared crate's data models.
pub mod model {
    pub use codetracer_trace_filter::model::{
        ExecDirective, FilterMeta, FilterSource, FilterSummary, FilterSummaryEntry, IoConfig,
        IoStream, ScopeRule, TraceFilterConfig, ValueAction, ValuePattern,
    };
}

/// Re-export of the shared crate's configuration façade.  Existing call
/// sites reference `crate::trace_filter::config::{TraceFilterConfig, ...}`.
pub mod config {
    pub use codetracer_trace_filter::config::{
        ExecDirective, FilterMeta, FilterSource, FilterSummary, FilterSummaryEntry, IoConfig,
        IoStream, ScopeRule, TraceFilterConfig, ValueAction, ValuePattern,
    };
}

use codetracer_trace_filter::error::{ErrorCode as FilterErrorCode, FilterError};
use recorder_errors::{usage, ErrorCode, RecorderError};

/// Convert a [`FilterError`] from the shared crate into a `RecorderError`
/// understood by the recorder's error facade.
pub(crate) fn convert_filter_error(err: FilterError) -> RecorderError {
    // The shared crate exposes a small set of error codes; map them onto
    // the recorder's richer taxonomy.  `InvalidPolicyValue` covers both
    // schema parse failures and unsupported schema versions per spec § 11
    // because the recorder's existing `InvalidPolicyValue` already covers
    // the "user-supplied filter rejected" case.
    let code = match err.code {
        FilterErrorCode::InvalidPolicyValue => ErrorCode::InvalidPolicyValue,
        FilterErrorCode::Io => ErrorCode::Io,
        FilterErrorCode::UnsupportedSchemaVersion => ErrorCode::InvalidPolicyValue,
    };
    usage!(code, "{}", err.message)
}
