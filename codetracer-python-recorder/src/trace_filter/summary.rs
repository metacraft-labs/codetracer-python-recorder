//! Trace filter summaries used for metadata embedding.

use crate::trace_filter::model::{FilterSource, FilterSummary, FilterSummaryEntry};

/// Build a summary object from the resolved filter sources list.
pub fn build_summary(sources: &[FilterSource]) -> FilterSummary {
    let entries = sources
        .iter()
        .map(|source| FilterSummaryEntry {
            path: source.path.clone(),
            sha256: source.sha256.clone(),
            name: source.meta.name.clone(),
            version: source.meta.version,
        })
        .collect();
    FilterSummary { entries }
}
