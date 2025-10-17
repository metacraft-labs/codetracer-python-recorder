//! Trace filter data models (directives, rules, summaries).

use crate::trace_filter::selector::Selector;
use crate::trace_filter::summary;
use std::path::PathBuf;

/// Scope-level execution directive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecDirective {
    Trace,
    Skip,
}

impl ExecDirective {
    pub(crate) fn parse(token: &str) -> Option<Self> {
        match token {
            "trace" => Some(ExecDirective::Trace),
            "skip" => Some(ExecDirective::Skip),
            _ => None,
        }
    }
}

/// Value-level capture directive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueAction {
    Allow,
    Redact,
    Drop,
}

impl ValueAction {
    pub(crate) fn parse(token: &str) -> Option<Self> {
        match token {
            "allow" => Some(ValueAction::Allow),
            "redact" => Some(ValueAction::Redact),
            "drop" => Some(ValueAction::Drop),
            // Backwards compatibility for deprecated `deny`.
            "deny" => Some(ValueAction::Redact),
            _ => None,
        }
    }
}

/// IO streams that can be captured in addition to scope/value rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IoStream {
    Stdout,
    Stderr,
    Stdin,
    Files,
}

impl IoStream {
    pub(crate) fn parse(token: &str) -> Option<Self> {
        match token {
            "stdout" => Some(IoStream::Stdout),
            "stderr" => Some(IoStream::Stderr),
            "stdin" => Some(IoStream::Stdin),
            "files" => Some(IoStream::Files),
            _ => None,
        }
    }
}

/// Metadata describing the source filter file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterMeta {
    pub name: String,
    pub version: u32,
    pub description: Option<String>,
    pub labels: Vec<String>,
}

/// IO capture configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IoConfig {
    pub capture: bool,
    pub streams: Vec<IoStream>,
}

impl Default for IoConfig {
    fn default() -> Self {
        IoConfig {
            capture: false,
            streams: Vec::new(),
        }
    }
}

/// Value pattern applied within a scope rule.
#[derive(Debug, Clone)]
pub struct ValuePattern {
    pub selector: Selector,
    pub action: ValueAction,
    pub reason: Option<String>,
    pub source_id: usize,
}

/// Scope rule constructed from the flattened configuration chain.
#[derive(Debug, Clone)]
pub struct ScopeRule {
    pub selector: Selector,
    pub exec: Option<ExecDirective>,
    pub value_default: Option<ValueAction>,
    pub value_patterns: Vec<ValuePattern>,
    pub reason: Option<String>,
    pub source_id: usize,
}

/// Source information for each filter file participating in the chain.
#[derive(Debug, Clone)]
pub struct FilterSource {
    pub path: PathBuf,
    pub sha256: String,
    pub project_root: PathBuf,
    pub meta: FilterMeta,
}

/// Summary used for embedding in trace metadata.
#[derive(Debug, Clone)]
pub struct FilterSummary {
    pub entries: Vec<FilterSummaryEntry>,
}

/// Single entry in the filter summary.
#[derive(Debug, Clone)]
pub struct FilterSummaryEntry {
    pub path: PathBuf,
    pub sha256: String,
    pub name: String,
    pub version: u32,
}

/// Fully resolved filter configuration ready for runtime consumption.
#[derive(Debug, Clone)]
pub struct TraceFilterConfig {
    pub(crate) default_exec: ExecDirective,
    pub(crate) default_value_action: ValueAction,
    pub(crate) io: IoConfig,
    pub(crate) rules: Vec<ScopeRule>,
    pub(crate) sources: Vec<FilterSource>,
}

impl TraceFilterConfig {
    /// Default execution directive applied before scope rules run.
    pub fn default_exec(&self) -> ExecDirective {
        self.default_exec
    }

    /// Default value action applied before rule-specific overrides.
    pub fn default_value_action(&self) -> ValueAction {
        self.default_value_action
    }

    /// IO capture configuration associated with the composed filter chain.
    pub fn io(&self) -> &IoConfig {
        &self.io
    }

    /// Flattened scope rules in execution order.
    pub fn rules(&self) -> &[ScopeRule] {
        &self.rules
    }

    /// Source filter metadata used for embedding in trace output.
    pub fn sources(&self) -> &[FilterSource] {
        &self.sources
    }

    /// Helper producing a summary used by metadata writers.
    pub fn summary(&self) -> FilterSummary {
        summary::build_summary(&self.sources)
    }
}
