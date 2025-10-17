//! Filter configuration facade: composes inline and file-based sources into a
//! resolved [`TraceFilterConfig`](crate::trace_filter::model::TraceFilterConfig).
//!
//! The implementation follows the schema defined in
//! `design-docs/US0028 - Configurable Python trace filters.md`.

pub use crate::trace_filter::model::{
    ExecDirective, FilterMeta, FilterSource, FilterSummary, FilterSummaryEntry, IoConfig, IoStream,
    ScopeRule, TraceFilterConfig, ValueAction, ValuePattern,
};

use crate::trace_filter::loader::ConfigAggregator;
use recorder_errors::{usage, ErrorCode, RecorderResult};
use std::path::PathBuf;

impl TraceFilterConfig {
    /// Load and compose filters from the provided paths.
    pub fn from_paths(paths: &[PathBuf]) -> RecorderResult<Self> {
        Self::from_inline_and_paths(&[], paths)
    }

    /// Load and compose filters from inline TOML sources combined with paths.
    ///
    /// Inline entries are ingested first in the order provided, followed by files.
    pub fn from_inline_and_paths(
        inline: &[(&str, &str)],
        paths: &[PathBuf],
    ) -> RecorderResult<Self> {
        if inline.is_empty() && paths.is_empty() {
            return Err(usage!(
                ErrorCode::InvalidPolicyValue,
                "no trace filter sources supplied"
            ));
        }

        let mut aggregator = ConfigAggregator::default();
        for (label, contents) in inline {
            aggregator.ingest_inline(label, contents)?;
        }
        for path in paths {
            aggregator.ingest_file(path)?;
        }

        aggregator.finish()
    }
}
