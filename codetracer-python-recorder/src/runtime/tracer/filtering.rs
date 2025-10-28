//! Trace filter cache management for `RuntimeTracer`.

use crate::code_object::CodeObjectWrapper;
use crate::logging::{record_dropped_event, with_error_code};
use crate::runtime::io_capture::ScopedMuteIoCapture;
use crate::runtime::value_capture::ValueFilterStats;
use crate::trace_filter::engine::{ExecDecision, ScopeResolution, TraceFilterEngine, ValueKind};
use pyo3::prelude::*;
use recorder_errors::ErrorCode;
use serde_json::{self, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Filtering outcome for a code object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TraceDecision {
    Trace,
    SkipAndDisable,
}

/// Coordinates trace filter execution, caching, and telemetry.
pub(crate) struct FilterCoordinator {
    engine: Option<Arc<TraceFilterEngine>>,
    ignored_code_ids: HashSet<usize>,
    scope_cache: HashMap<usize, Arc<ScopeResolution>>,
    module_name_hints: HashMap<usize, String>,
    stats: FilterStats,
}

impl FilterCoordinator {
    pub(crate) fn new(engine: Option<Arc<TraceFilterEngine>>) -> Self {
        Self {
            engine,
            ignored_code_ids: HashSet::new(),
            scope_cache: HashMap::new(),
            module_name_hints: HashMap::new(),
            stats: FilterStats::default(),
        }
    }

    pub(crate) fn engine(&self) -> Option<&Arc<TraceFilterEngine>> {
        self.engine.as_ref()
    }

    pub(crate) fn cached_resolution(&self, code_id: usize) -> Option<Arc<ScopeResolution>> {
        self.scope_cache.get(&code_id).cloned()
    }

    pub(crate) fn summary_json(&self) -> serde_json::Value {
        self.stats.summary_json()
    }

    pub(crate) fn values_mut(&mut self) -> &mut ValueFilterStats {
        self.stats.values_mut()
    }

    pub(crate) fn set_module_name_hint(&mut self, code_id: usize, hint: Option<String>) {
        match hint {
            Some(value) => {
                self.module_name_hints.insert(code_id, value);
            }
            None => {
                self.module_name_hints.remove(&code_id);
            }
        }
    }

    pub(crate) fn module_name_hint(&self, code_id: usize) -> Option<String> {
        self.module_name_hints.get(&code_id).cloned()
    }

    pub(crate) fn clear_caches(&mut self) {
        self.ignored_code_ids.clear();
        self.scope_cache.clear();
    }

    pub(crate) fn reset(&mut self) {
        self.clear_caches();
        self.stats.reset();
    }

    pub(crate) fn decide(&mut self, py: Python<'_>, code: &CodeObjectWrapper) -> TraceDecision {
        let code_id = code.id();
        if self.ignored_code_ids.contains(&code_id) {
            return TraceDecision::SkipAndDisable;
        }

        if let Some(resolution) = self.resolve(py, code) {
            if resolution.exec() == ExecDecision::Skip {
                self.mark_ignored(code_id);
                self.stats.record_skip();
                record_dropped_event("filter_scope_skip");
                return TraceDecision::SkipAndDisable;
            }
        }

        let filename = match code.filename(py) {
            Ok(name) => name,
            Err(err) => {
                with_error_code(ErrorCode::Io, || {
                    let _mute = ScopedMuteIoCapture::new();
                    log::error!("failed to resolve code filename: {err}");
                });
                record_dropped_event("filename_lookup_failed");
                self.mark_ignored(code_id);
                return TraceDecision::SkipAndDisable;
            }
        };

        if is_real_filename(filename) {
            TraceDecision::Trace
        } else {
            record_dropped_event("synthetic_filename");
            self.mark_ignored(code_id);
            TraceDecision::SkipAndDisable
        }
    }

    fn resolve(
        &mut self,
        py: Python<'_>,
        code: &CodeObjectWrapper,
    ) -> Option<Arc<ScopeResolution>> {
        let engine = self.engine.as_ref()?;
        let code_id = code.id();

        if let Some(existing) = self.scope_cache.get(&code_id) {
            return Some(existing.clone());
        }

        let hint = self.module_name_hints.get(&code_id).map(|s| s.as_str());
        match engine.resolve(py, code, hint) {
            Ok(resolution) => {
                if resolution.exec() == ExecDecision::Trace {
                    self.scope_cache.insert(code_id, Arc::clone(&resolution));
                } else {
                    self.scope_cache.remove(&code_id);
                }
                Some(resolution)
            }
            Err(err) => {
                let message = err.to_string();
                let error_code = err.code;
                with_error_code(error_code, || {
                    let _mute = ScopedMuteIoCapture::new();
                    log::error!(
                        "[RuntimeTracer] trace filter resolution failed for code id {}: {}",
                        code_id,
                        message
                    );
                });
                record_dropped_event("filter_resolution_error");
                None
            }
        }
    }

    fn mark_ignored(&mut self, code_id: usize) {
        self.scope_cache.remove(&code_id);
        self.ignored_code_ids.insert(code_id);
        self.module_name_hints.remove(&code_id);
    }
}

/// Return true when the filename refers to a concrete source file.
pub(crate) fn is_real_filename(filename: &str) -> bool {
    let trimmed = filename.trim();
    !(trimmed.starts_with('<') && trimmed.ends_with('>'))
}

#[derive(Debug, Default)]
struct FilterStats {
    skipped_scopes: u64,
    values: ValueFilterStats,
}

impl FilterStats {
    fn record_skip(&mut self) {
        self.skipped_scopes += 1;
    }

    fn values_mut(&mut self) -> &mut ValueFilterStats {
        &mut self.values
    }

    fn reset(&mut self) {
        self.skipped_scopes = 0;
        self.values = ValueFilterStats::default();
    }

    fn summary_json(&self) -> serde_json::Value {
        let mut redactions = serde_json::Map::new();
        let mut drops = serde_json::Map::new();
        for kind in ValueKind::ALL {
            redactions.insert(
                kind.label().to_string(),
                json!(self.values.redacted_count(kind)),
            );
            drops.insert(
                kind.label().to_string(),
                json!(self.values.dropped_count(kind)),
            );
        }
        json!({
            "scopes_skipped": self.skipped_scopes,
            "value_redactions": serde_json::Value::Object(redactions),
            "value_drops": serde_json::Value::Object(drops),
        })
    }
}
