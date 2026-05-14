//! Trace filter cache management for `RuntimeTracer`.
//!
//! Post-TF-M6 the per-code-object resolution cache lives inside CPython's
//! native `co_extra` slot (see `crate::trace_filter::engine`).  This module
//! therefore no longer maintains its own `HashMap<CodeId, Arc<ScopeResolution>>`
//! mirror; it only tracks the coordinator-level decisions that don't have a
//! cleaner home (synthetic-filename ignore set, module-name hints, telemetry
//! counters) and delegates every scope resolution to the engine, which
//! reads/writes `co_extra` directly.

use crate::code_object::CodeObjectWrapper;
use crate::logging::{record_dropped_event, with_error_code};
use crate::runtime::io_capture::ScopedMuteIoCapture;
use crate::runtime::value_capture::ValueFilterStats;
use crate::trace_filter::engine::{ExecDecision, ScopeResolution, TraceFilterEngine, ValueKind};
use pyo3::prelude::*;
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
    /// Codes the tracer has permanently disabled (synthetic filenames,
    /// filter-skipped scopes, attribute-lookup failures, ...). Lookup is
    /// O(1) and the set is small (one entry per ignored code object,
    /// freed when the tracer resets).
    ignored_code_ids: HashSet<usize>,
    /// Frame-globals `__name__` captured at `on_py_start`. Forwarded to
    /// the classifier so package selectors resolve correctly even when
    /// the filename doesn't lie under a `__init__.py`-style package tree.
    module_name_hints: HashMap<usize, String>,
    stats: FilterStats,
}

impl FilterCoordinator {
    pub(crate) fn new(engine: Option<Arc<TraceFilterEngine>>) -> Self {
        Self {
            engine,
            ignored_code_ids: HashSet::new(),
            module_name_hints: HashMap::new(),
            stats: FilterStats::default(),
        }
    }

    pub(crate) fn engine(&self) -> Option<&Arc<TraceFilterEngine>> {
        self.engine.as_ref()
    }

    /// Fast-path resolution lookup intended for per-event callers.
    ///
    /// Per spec § 6 this MUST avoid a hash lookup keyed by `code_id`.
    /// The lookup is delegated to the engine, which reads CPython's
    /// `co_extra` slot — one CPython call, no hashes.
    ///
    /// Returns `None` when filtering is disabled or when the code object
    /// has not yet been classified (which in practice never happens after
    /// `on_py_start` runs, because that path always calls `decide` which
    /// populates the cache).
    pub(crate) fn cached_resolution(
        &self,
        py: Python<'_>,
        code: &CodeObjectWrapper,
    ) -> Option<Arc<ScopeResolution>> {
        let engine = self.engine.as_ref()?;
        // `engine.resolve` returns a cached entry from `co_extra` on hit;
        // on miss it classifies and stores.  Either way the cost is one
        // `_PyCode_GetExtra` call on the hot path.
        let hint = self.module_name_hints.get(&code.id()).map(|s| s.as_str());
        match engine.resolve(py, code, hint) {
            Ok(resolution) => Some(resolution),
            Err(_) => None,
        }
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
        // Note: the per-code-object resolutions stashed in `co_extra` are
        // owned by CPython and freed via the registered `freefunc` when
        // the code object itself is destroyed. We deliberately do not
        // attempt to walk every live code object to clear those slots
        // here — that would be O(every Python code object ever loaded).
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
                with_error_code(recorder_errors::ErrorCode::Io, || {
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
        let hint = self.module_name_hints.get(&code_id).map(|s| s.as_str());
        match engine.resolve(py, code, hint) {
            Ok(resolution) => Some(resolution),
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
