//! Filter configuration loader that parses TOML files, resolves inheritance, and
//! prepares flattened scope/value rules for the runtime engine.
//!
//! The implementation follows the schema defined in
//! `design-docs/US0028 - Configurable Python trace filters.md`.

use crate::trace_filter::selector::{MatchType, Selector, SelectorKind};
use recorder_errors::{usage, ErrorCode, RecorderResult};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Scope-level execution directive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecDirective {
    Trace,
    Skip,
}

impl ExecDirective {
    fn parse(token: &str) -> Option<Self> {
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
    Deny,
}

impl ValueAction {
    fn parse(token: &str) -> Option<Self> {
        match token {
            "allow" => Some(ValueAction::Allow),
            "deny" => Some(ValueAction::Deny),
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
    fn parse(token: &str) -> Option<Self> {
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
    default_exec: ExecDirective,
    default_value_action: ValueAction,
    io: IoConfig,
    rules: Vec<ScopeRule>,
    sources: Vec<FilterSource>,
}

impl TraceFilterConfig {
    /// Load and compose filters from the provided paths.
    pub fn from_paths(paths: &[PathBuf]) -> RecorderResult<Self> {
        if paths.is_empty() {
            return Err(usage!(
                ErrorCode::InvalidPolicyValue,
                "no trace filter paths supplied"
            ));
        }

        let mut aggregator = ConfigAggregator::default();
        for path in paths {
            aggregator.ingest_file(path)?;
        }

        aggregator.finish()
    }

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
        let entries = self
            .sources
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
}

#[derive(Default)]
struct ConfigAggregator {
    default_exec: Option<ExecDirective>,
    default_value_action: Option<ValueAction>,
    io: Option<IoConfig>,
    rules: Vec<ScopeRule>,
    sources: Vec<FilterSource>,
}

impl ConfigAggregator {
    fn ingest_file(&mut self, path: &Path) -> RecorderResult<()> {
        let contents = fs::read_to_string(path).map_err(|err| {
            usage!(
                ErrorCode::InvalidPolicyValue,
                "failed to read trace filter '{}': {}",
                path.display(),
                err
            )
        })?;

        let checksum = calculate_sha256(&contents);
        let raw: RawFilterFile = toml::from_str(&contents).map_err(|err| {
            usage!(
                ErrorCode::InvalidPolicyValue,
                "failed to parse trace filter '{}': {}",
                path.display(),
                err
            )
        })?;

        let project_root = detect_project_root(path);
        let source_index = self.sources.len();
        self.sources.push(FilterSource {
            path: path.to_path_buf(),
            sha256: checksum,
            project_root: project_root.clone(),
            meta: parse_meta(&raw.meta, path)?,
        });

        let defaults = resolve_defaults(
            &raw.scope,
            path,
            self.default_exec,
            self.default_value_action,
        )?;
        if let Some(exec) = defaults.exec {
            self.default_exec = Some(exec);
        }
        if let Some(value_action) = defaults.value_action {
            self.default_value_action = Some(value_action);
        }

        if let Some(io) = parse_io(raw.io.as_ref(), path)? {
            self.io = Some(io);
        }

        let rules = parse_rules(
            raw.scope.rules.as_deref().unwrap_or_default(),
            path,
            &project_root,
            source_index,
        )?;
        self.rules.extend(rules);

        Ok(())
    }

    fn finish(self) -> RecorderResult<TraceFilterConfig> {
        let default_exec = self.default_exec.ok_or_else(|| {
            usage!(
                ErrorCode::InvalidPolicyValue,
                "composed filters never set 'scope.default_exec'"
            )
        })?;
        let default_value_action = self.default_value_action.ok_or_else(|| {
            usage!(
                ErrorCode::InvalidPolicyValue,
                "composed filters never set 'scope.default_value_action'"
            )
        })?;

        let io = self.io.unwrap_or_default();

        Ok(TraceFilterConfig {
            default_exec,
            default_value_action,
            io,
            rules: self.rules,
            sources: self.sources,
        })
    }
}

fn calculate_sha256(contents: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(contents.as_bytes());
    let digest = hasher.finalize();
    format!("{:x}", digest)
}

fn detect_project_root(path: &Path) -> PathBuf {
    let mut current = path.parent();
    while let Some(dir) = current {
        if dir.file_name().and_then(|name| name.to_str()) == Some(".codetracer") {
            return dir
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| dir.to_path_buf());
        }
        current = dir.parent();
    }
    path.parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn parse_meta(raw: &RawMeta, path: &Path) -> RecorderResult<FilterMeta> {
    if raw.name.trim().is_empty() {
        return Err(usage!(
            ErrorCode::InvalidPolicyValue,
            "'meta.name' cannot be empty in '{}'",
            path.display()
        ));
    }
    if raw.version < 1 {
        return Err(usage!(
            ErrorCode::InvalidPolicyValue,
            "'meta.version' must be >= 1 in '{}'",
            path.display()
        ));
    }

    let mut labels = Vec::new();
    let mut seen = HashSet::new();
    for label in &raw.labels {
        if seen.insert(label) {
            labels.push(label.clone());
        }
    }

    Ok(FilterMeta {
        name: raw.name.clone(),
        version: raw.version as u32,
        description: raw.description.clone(),
        labels,
    })
}

struct ResolvedDefaults {
    exec: Option<ExecDirective>,
    value_action: Option<ValueAction>,
}

fn resolve_defaults(
    scope: &RawScope,
    path: &Path,
    current_exec: Option<ExecDirective>,
    current_value_action: Option<ValueAction>,
) -> RecorderResult<ResolvedDefaults> {
    let exec = parse_default_exec(&scope.default_exec, path, current_exec)?;
    let value_action =
        parse_default_value_action(&scope.default_value_action, path, current_value_action)?;
    Ok(ResolvedDefaults { exec, value_action })
}

fn parse_default_exec(
    token: &str,
    path: &Path,
    current_exec: Option<ExecDirective>,
) -> RecorderResult<Option<ExecDirective>> {
    match token {
        "inherit" => {
            if current_exec.is_none() {
                return Err(usage!(
                    ErrorCode::InvalidPolicyValue,
                    "'scope.default_exec' in '{}' cannot inherit without a previous filter",
                    path.display()
                ));
            }
            Ok(None)
        }
        _ => ExecDirective::parse(token)
            .ok_or_else(|| {
                usage!(
                    ErrorCode::InvalidPolicyValue,
                    "unsupported value '{}' for 'scope.default_exec' in '{}'",
                    token,
                    path.display()
                )
            })
            .map(Some),
    }
}

fn parse_default_value_action(
    token: &str,
    path: &Path,
    current_value_action: Option<ValueAction>,
) -> RecorderResult<Option<ValueAction>> {
    match token {
        "inherit" => {
            if current_value_action.is_none() {
                return Err(usage!(
                    ErrorCode::InvalidPolicyValue,
                    "'scope.default_value_action' in '{}' cannot inherit without a previous filter",
                    path.display()
                ));
            }
            Ok(None)
        }
        _ => ValueAction::parse(token)
            .ok_or_else(|| {
                usage!(
                    ErrorCode::InvalidPolicyValue,
                    "unsupported value '{}' for 'scope.default_value_action' in '{}'",
                    token,
                    path.display()
                )
            })
            .map(Some),
    }
}

fn parse_io(raw: Option<&RawIo>, path: &Path) -> RecorderResult<Option<IoConfig>> {
    let Some(raw) = raw else {
        return Ok(None);
    };

    let capture = raw.capture.unwrap_or(false);
    let streams = match raw.streams.as_ref() {
        Some(values) => {
            let mut parsed = Vec::new();
            let mut seen = HashSet::new();
            for value in values {
                let stream = IoStream::parse(value).ok_or_else(|| {
                    usage!(
                        ErrorCode::InvalidPolicyValue,
                        "unsupported IO stream '{}' in '{}'",
                        value,
                        path.display()
                    )
                })?;
                if seen.insert(stream) {
                    parsed.push(stream);
                }
            }
            parsed
        }
        None => Vec::new(),
    };

    if capture && streams.is_empty() {
        return Err(usage!(
            ErrorCode::InvalidPolicyValue,
            "'io.streams' must be provided when 'io.capture = true' in '{}'",
            path.display()
        ));
    }
    if let Some(modes) = raw.modes.as_ref() {
        if !modes.is_empty() {
            return Err(usage!(
                ErrorCode::InvalidPolicyValue,
                "'io.modes' is reserved and must be empty in '{}'",
                path.display()
            ));
        }
    }

    Ok(Some(IoConfig { capture, streams }))
}

fn parse_rules(
    raw_rules: &[RawScopeRule],
    path: &Path,
    project_root: &Path,
    source_id: usize,
) -> RecorderResult<Vec<ScopeRule>> {
    let mut rules = Vec::new();
    for (idx, raw_rule) in raw_rules.iter().enumerate() {
        let location = format!("{} scope.rules[{}]", path.display(), idx);
        let selector =
            Selector::parse(&raw_rule.selector, &SCOPE_SELECTOR_KINDS).map_err(|err| {
                usage!(
                    ErrorCode::InvalidPolicyValue,
                    "invalid scope selector in {}: {}",
                    location,
                    err
                )
            })?;
        let selector = normalize_scope_selector(selector, project_root, &location)?;

        let exec = match raw_rule.exec.as_deref() {
            None | Some("inherit") => None,
            Some(value) => Some(ExecDirective::parse(value).ok_or_else(|| {
                usage!(
                    ErrorCode::InvalidPolicyValue,
                    "unsupported value '{}' for 'exec' in {}",
                    value,
                    location
                )
            })?),
        };

        let value_default = match raw_rule.value_default.as_deref() {
            None | Some("inherit") => None,
            Some(value) => Some(ValueAction::parse(value).ok_or_else(|| {
                usage!(
                    ErrorCode::InvalidPolicyValue,
                    "unsupported value '{}' for 'value_default' in {}",
                    value,
                    location
                )
            })?),
        };

        let mut value_patterns = Vec::new();
        if let Some(patterns) = raw_rule.value_patterns.as_ref() {
            for (pidx, pattern) in patterns.iter().enumerate() {
                let pattern_location = format!("{} value_patterns[{}]", location, pidx);
                let selector =
                    Selector::parse(&pattern.selector, &VALUE_SELECTOR_KINDS).map_err(|err| {
                        usage!(
                            ErrorCode::InvalidPolicyValue,
                            "invalid value selector in {}: {}",
                            pattern_location,
                            err
                        )
                    })?;
                let action = ValueAction::parse(pattern.action.as_str()).ok_or_else(|| {
                    usage!(
                        ErrorCode::InvalidPolicyValue,
                        "unsupported value '{}' for 'action' in {}",
                        pattern.action,
                        pattern_location
                    )
                })?;

                value_patterns.push(ValuePattern {
                    selector,
                    action,
                    reason: pattern.reason.clone(),
                    source_id,
                });
            }
        }

        rules.push(ScopeRule {
            selector,
            exec,
            value_default,
            value_patterns,
            reason: raw_rule.reason.clone(),
            source_id,
        });
    }
    Ok(rules)
}

fn normalize_scope_selector(
    selector: Selector,
    project_root: &Path,
    location: &str,
) -> RecorderResult<Selector> {
    if selector.kind() != SelectorKind::File {
        return Ok(selector);
    }

    let normalized_pattern = normalize_file_pattern(
        selector.pattern(),
        selector.match_type(),
        project_root,
        location,
    )?;
    if normalized_pattern == selector.pattern() {
        return Ok(selector);
    }

    let raw = match selector.match_type() {
        MatchType::Glob => format!("file:{}", normalized_pattern),
        MatchType::Literal => format!("file:literal:{}", normalized_pattern),
        MatchType::Regex => format!("file:regex:{}", normalized_pattern),
    };
    Selector::parse(&raw, &SCOPE_SELECTOR_KINDS).map_err(|err| {
        usage!(
            ErrorCode::InvalidPolicyValue,
            "failed to normalise file selector in {}: {}",
            location,
            err
        )
    })
}

fn normalize_file_pattern(
    pattern: &str,
    match_type: MatchType,
    project_root: &Path,
    location: &str,
) -> RecorderResult<String> {
    match match_type {
        MatchType::Literal => normalize_literal_path(pattern, project_root, location),
        MatchType::Glob => normalize_glob_pattern(pattern, project_root),
        MatchType::Regex => Ok(pattern.to_string()),
    }
}

fn normalize_literal_path(
    pattern: &str,
    project_root: &Path,
    location: &str,
) -> RecorderResult<String> {
    let path = Path::new(pattern);
    let relative = if path.is_absolute() {
        path.strip_prefix(project_root)
            .map_err(|_| {
                usage!(
                    ErrorCode::InvalidPolicyValue,
                    "file selector '{}' in {} must reside within project root '{}'",
                    pattern,
                    location,
                    project_root.display()
                )
            })?
            .to_path_buf()
    } else {
        path.to_path_buf()
    };

    let normalized = normalize_components(&relative, pattern, location)?;
    Ok(pathbuf_to_posix(&normalized))
}

fn normalize_components(path: &Path, raw: &str, location: &str) -> RecorderResult<PathBuf> {
    let mut normalised = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => continue,
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalised.pop() {
                    return Err(usage!(
                        ErrorCode::InvalidPolicyValue,
                        "file selector '{}' in {} escapes the project root",
                        raw,
                        location
                    ));
                }
            }
            Component::Normal(part) => normalised.push(part),
        }
    }
    Ok(normalised)
}

fn normalize_glob_pattern(pattern: &str, project_root: &Path) -> RecorderResult<String> {
    let mut replaced = pattern.replace('\\', "/");
    while replaced.starts_with("./") {
        replaced = replaced[2..].to_string();
    }

    let trimmed = replaced.trim_start_matches('/');
    let root = pathbuf_to_posix(project_root);
    if root.is_empty() {
        return Ok(trimmed.to_string());
    }

    let root_with_slash = format!("{}/", root);
    if trimmed.starts_with(&root_with_slash) {
        Ok(trimmed[root_with_slash.len()..].to_string())
    } else if trimmed == root {
        Ok(String::new())
    } else {
        Ok(trimmed.to_string())
    }
}

fn pathbuf_to_posix(path: &Path) -> String {
    let mut parts = Vec::new();
    for component in path.components() {
        if let Component::Normal(part) = component {
            parts.push(part.to_string_lossy());
        }
    }
    parts.join("/")
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFilterFile {
    meta: RawMeta,
    #[serde(default)]
    io: Option<RawIo>,
    scope: RawScope,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMeta {
    name: String,
    version: u32,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    labels: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIo {
    #[serde(default)]
    capture: Option<bool>,
    #[serde(default)]
    streams: Option<Vec<String>>,
    #[serde(default)]
    modes: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawScope {
    default_exec: String,
    default_value_action: String,
    #[serde(default)]
    rules: Option<Vec<RawScopeRule>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawScopeRule {
    selector: String,
    #[serde(default)]
    exec: Option<String>,
    #[serde(default)]
    value_default: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    value_patterns: Option<Vec<RawValuePattern>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawValuePattern {
    selector: String,
    action: String,
    #[serde(default)]
    reason: Option<String>,
}

const SCOPE_SELECTOR_KINDS: [SelectorKind; 3] = [
    SelectorKind::Package,
    SelectorKind::File,
    SelectorKind::Object,
];

const VALUE_SELECTOR_KINDS: [SelectorKind; 5] = [
    SelectorKind::Local,
    SelectorKind::Global,
    SelectorKind::Arg,
    SelectorKind::Return,
    SelectorKind::Attr,
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn composes_filters_and_resolves_inheritance() -> RecorderResult<()> {
        let temp = tempdir().expect("temp dir");
        let project_root = temp.path();
        let filters_dir = project_root.join(".codetracer");
        fs::create_dir(&filters_dir).unwrap();
        fs::create_dir_all(project_root.join("app")).unwrap();

        let base_path = filters_dir.join("base.toml");
        write_filter(
            &base_path,
            r#"
            [meta]
            name = "base"
            version = 1

            [scope]
            default_exec = "trace"
            default_value_action = "deny"

            [[scope.rules]]
            selector = "pkg:my_app.core.*"
            exec = "trace"
            value_default = "allow"

            [[scope.rules.value_patterns]]
            selector = "local:literal:user"
            action = "allow"

            [io]
            capture = false
            "#,
        );

        let overrides_path = filters_dir.join("overrides.toml");
        let literal_path = project_root
            .join(".codetracer")
            .join("..")
            .join("app")
            .join("__init__.py");
        let overrides = format!(
            r#"
            [meta]
            name = "overrides"
            version = 1

            [scope]
            default_exec = "inherit"
            default_value_action = "inherit"

            [[scope.rules]]
            selector = "file:literal:{literal}"
            exec = "inherit"
            value_default = "deny"

            [[scope.rules.value_patterns]]
            selector = "arg:password"
            action = "deny"

            [io]
            capture = true
            streams = ["stdout", "stderr"]
            "#,
            literal = literal_path.to_string_lossy()
        );
        write_filter(&overrides_path, overrides.as_str());

        let config = TraceFilterConfig::from_paths(&[base_path.clone(), overrides_path.clone()])?;

        assert_eq!(config.default_exec(), ExecDirective::Trace);
        assert_eq!(config.default_value_action(), ValueAction::Deny);
        assert_eq!(config.io().capture, true);
        assert_eq!(
            config.io().streams,
            vec![IoStream::Stdout, IoStream::Stderr]
        );

        assert_eq!(config.rules().len(), 2);
        let file_rule = &config.rules()[1];
        assert!(matches!(file_rule.exec, None));
        assert_eq!(file_rule.value_default, Some(ValueAction::Deny));
        assert_eq!(file_rule.value_patterns.len(), 1);
        assert_eq!(file_rule.value_patterns[0].selector.raw(), "arg:password");
        assert_eq!(
            file_rule.selector.pattern(),
            "app/__init__.py",
            "absolute literal path normalised relative to project root"
        );

        let summary = config.summary();
        assert_eq!(summary.entries.len(), 2);
        assert_eq!(summary.entries[0].name, "base");
        assert_eq!(summary.entries[1].name, "overrides");

        Ok(())
    }

    #[test]
    fn rejects_unknown_keys() {
        let temp = tempdir().expect("temp dir");
        let project_root = temp.path();
        let filters_dir = project_root.join(".codetracer");
        fs::create_dir(&filters_dir).unwrap();
        let path = filters_dir.join("invalid.toml");
        write_filter(
            &path,
            r#"
            [meta]
            name = "invalid"
            version = 1
            extra = "nope"

            [scope]
            default_exec = "trace"
            default_value_action = "deny"
            "#,
        );

        let err = TraceFilterConfig::from_paths(&[path]).expect_err("expected failure");
        assert_eq!(err.code, ErrorCode::InvalidPolicyValue);
    }

    #[test]
    fn rejects_inherit_without_base() {
        let temp = tempdir().expect("temp dir");
        let project_root = temp.path();
        let filters_dir = project_root.join(".codetracer");
        fs::create_dir(&filters_dir).unwrap();
        let path = filters_dir.join("empty.toml");
        write_filter(
            &path,
            r#"
            [meta]
            name = "empty"
            version = 1

            [scope]
            default_exec = "inherit"
            default_value_action = "inherit"
            "#,
        );

        let err = TraceFilterConfig::from_paths(&[path]).expect_err("expected failure");
        assert_eq!(err.code, ErrorCode::InvalidPolicyValue);
    }

    #[test]
    fn rejects_invalid_stream_value() {
        let temp = tempdir().expect("temp dir");
        let project_root = temp.path();
        let filters_dir = project_root.join(".codetracer");
        fs::create_dir(&filters_dir).unwrap();
        let path = filters_dir.join("io.toml");
        write_filter(
            &path,
            r#"
            [meta]
            name = "io"
            version = 1

            [scope]
            default_exec = "trace"
            default_value_action = "allow"

            [io]
            capture = true
            streams = ["stdout", "invalid"]
            "#,
        );

        let err = TraceFilterConfig::from_paths(&[path]).expect_err("expected failure");
        assert_eq!(err.code, ErrorCode::InvalidPolicyValue);
    }

    fn write_filter(path: &Path, contents: &str) {
        let mut file = fs::File::create(path).unwrap();
        file.write_all(contents.trim_start().as_bytes()).unwrap();
    }
}
