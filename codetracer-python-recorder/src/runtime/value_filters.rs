//! Policy-aware helpers for value encoding.
//!
//! This module owns the bridge between the trace filter policy layer and the
//! value encoder. Callers provide the runtime policy and obtain either encoded
//! values, redaction sentinels, or `None` when the policy drops a value
//! altogether. The helpers also maintain telemetry counters so downstream
//! tooling can expose filtering statistics.

use crate::logging::record_dropped_event;
use crate::runtime::value_encoder::encode_value;
use crate::trace_filter::config::ValueAction;
use crate::trace_filter::engine::{ValueKind, ValuePolicy};
use pyo3::prelude::*;
use pyo3::types::PyAny;
use runtime_tracing::{NonStreamingTraceWriter, TraceWriter, TypeKind, ValueRecord};

/// Telemetry counters tracking value filtering outcomes.
#[derive(Debug, Default, Clone)]
pub struct ValueFilterStats {
    redacted: [u64; ValueKind::ALL.len()],
    dropped: [u64; ValueKind::ALL.len()],
}

impl ValueFilterStats {
    pub fn record_redaction(&mut self, kind: ValueKind) {
        self.redacted[kind.index()] += 1;
    }

    pub fn record_drop(&mut self, kind: ValueKind) {
        self.dropped[kind.index()] += 1;
    }

    pub fn redacted_count(&self, kind: ValueKind) -> u64 {
        self.redacted[kind.index()]
    }

    pub fn dropped_count(&self, kind: ValueKind) -> u64 {
        self.dropped[kind.index()]
    }
}

pub(crate) const REDACTED_SENTINEL: &str = "<redacted>";
pub(crate) const DROPPED_SENTINEL: &str = "<dropped>";

/// Apply the value policy for `candidate`. When the policy allows encoding,
/// delegates to [`encode_value`]. Redacted values materialise as error records
/// containing the sentinel text, while dropped values propagate `None`.
pub(crate) fn encode_with_policy<'py>(
    py: Python<'py>,
    writer: &mut NonStreamingTraceWriter,
    value: &Bound<'py, PyAny>,
    policy: Option<&ValuePolicy>,
    kind: ValueKind,
    candidate: &str,
    telemetry: Option<&mut ValueFilterStats>,
) -> Option<ValueRecord> {
    match policy.map(|p| p.decide(kind, candidate)) {
        Some(ValueAction::Redact) => {
            record_redaction(kind, candidate, telemetry);
            Some(redacted_value(writer))
        }
        Some(ValueAction::Drop) => {
            record_drop(kind, candidate, telemetry);
            None
        }
        _ => Some(encode_value(py, writer, value)),
    }
}

pub(crate) fn redacted_value(writer: &mut NonStreamingTraceWriter) -> ValueRecord {
    let ty = TraceWriter::ensure_type_id(writer, TypeKind::Raw, "Redacted");
    ValueRecord::Error {
        msg: REDACTED_SENTINEL.to_string(),
        type_id: ty,
    }
}

pub(crate) fn dropped_value(writer: &mut NonStreamingTraceWriter) -> ValueRecord {
    let ty = TraceWriter::ensure_type_id(writer, TypeKind::Raw, "Dropped");
    ValueRecord::Error {
        msg: DROPPED_SENTINEL.to_string(),
        type_id: ty,
    }
}

fn record_redaction(kind: ValueKind, candidate: &str, telemetry: Option<&mut ValueFilterStats>) {
    if let Some(stats) = telemetry {
        stats.record_redaction(kind);
    }
    let metric = match kind {
        ValueKind::Arg => "filter_value_redacted.arg",
        ValueKind::Local => "filter_value_redacted.local",
        ValueKind::Global => "filter_value_redacted.global",
        ValueKind::Return => "filter_value_redacted.return",
        ValueKind::Attr => "filter_value_redacted.attr",
    };
    record_dropped_event(metric);
    log::debug!("[RuntimeTracer] redacted {} '{}'", kind.label(), candidate);
}

fn record_drop(kind: ValueKind, candidate: &str, telemetry: Option<&mut ValueFilterStats>) {
    if let Some(stats) = telemetry {
        stats.record_drop(kind);
    }
    let metric = match kind {
        ValueKind::Arg => "filter_value_dropped.arg",
        ValueKind::Local => "filter_value_dropped.local",
        ValueKind::Global => "filter_value_dropped.global",
        ValueKind::Return => "filter_value_dropped.return",
        ValueKind::Attr => "filter_value_dropped.attr",
    };
    record_dropped_event(metric);
    log::debug!(
        "[RuntimeTracer] dropped {} '{}' from trace",
        kind.label(),
        candidate
    );
}
