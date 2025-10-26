//! Trace filter configuration loader (TOML ingestion, aggregation).

use crate::trace_filter::model::{
    ExecDirective, FilterMeta, FilterSource, IoConfig, IoStream, ScopeRule, TraceFilterConfig,
    ValueAction, ValuePattern,
};
use crate::trace_filter::selector::{MatchType, Selector, SelectorKind};
use recorder_errors::{usage, ErrorCode, RecorderResult};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Helper aggregating inline and file sources into a resolved configuration.
#[derive(Default)]
pub struct ConfigAggregator {
    default_exec: Option<ExecDirective>,
    default_value_action: Option<ValueAction>,
    io: Option<IoConfig>,
    rules: Vec<ScopeRule>,
    sources: Vec<FilterSource>,
}

impl ConfigAggregator {
    /// Ingest a filter from the filesystem.
    pub fn ingest_file(&mut self, path: &Path) -> RecorderResult<()> {
        let contents = fs::read_to_string(path).map_err(|err| {
            usage!(
                ErrorCode::InvalidPolicyValue,
                "failed to read trace filter '{}': {}",
                path.display(),
                err
            )
        })?;

        self.ingest_source(path, &contents)
    }

    /// Ingest an inline filter (used for builtin defaults).
    pub fn ingest_inline(&mut self, label: &str, contents: &str) -> RecorderResult<()> {
        let pseudo_path = PathBuf::from(format!("<inline:{label}>"));
        self.ingest_source(&pseudo_path, contents)
    }

    /// Finalise the aggregation, producing a resolved configuration.
    pub fn finish(self) -> RecorderResult<TraceFilterConfig> {
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

    fn ingest_source(&mut self, path: &Path, contents: &str) -> RecorderResult<()> {
        let checksum = calculate_sha256(contents);
        let raw: RawFilterFile = toml::from_str(contents).map_err(|err| {
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
}

pub(crate) fn calculate_sha256(contents: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(contents.as_bytes());
    let digest = hasher.finalize();
    format!("{:x}", digest)
}

pub(crate) fn detect_project_root(path: &Path) -> PathBuf {
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

pub(crate) fn parse_meta(raw: &RawMeta, path: &Path) -> RecorderResult<FilterMeta> {
    if raw.name.trim().is_empty() {
        return Err(usage!(
            ErrorCode::InvalidPolicyValue,
            "'meta.name' must not be empty in '{}'",
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

pub(crate) struct ResolvedDefaults {
    pub exec: Option<ExecDirective>,
    pub value_action: Option<ValueAction>,
}

pub(crate) fn resolve_defaults(
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

pub(crate) fn parse_default_exec(
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

pub(crate) fn parse_default_value_action(
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

pub(crate) fn parse_io(raw: Option<&RawIo>, path: &Path) -> RecorderResult<Option<IoConfig>> {
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

pub(crate) fn parse_rules(
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

                let action = ValueAction::parse(&pattern.action).ok_or_else(|| {
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

pub(crate) fn normalize_scope_selector(
    selector: Selector,
    project_root: &Path,
    location: &str,
) -> RecorderResult<Selector> {
    match selector.kind() {
        SelectorKind::File => {
            let pattern = selector.pattern();
            if pattern.starts_with("glob:") {
                let glob_pattern = &pattern["glob:".len()..];
                let normalized = normalize_glob_pattern(glob_pattern, project_root)?;
                rebuild_selector(selector.kind(), selector.match_type(), &normalized)
            } else {
                let path = Path::new(pattern);
                let normalized = normalize_file_selector(path, project_root, pattern, location)?;
                rebuild_selector(selector.kind(), selector.match_type(), &normalized)
            }
        }
        _ => Ok(selector),
    }
}

pub(crate) fn normalize_file_selector(
    path: &Path,
    project_root: &Path,
    pattern: &str,
    location: &str,
) -> RecorderResult<String> {
    let path = if path.is_absolute() {
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

    let normalized = normalize_components(&path, pattern, location)?;
    Ok(pathbuf_to_posix(&normalized))
}

pub(crate) fn normalize_components(
    path: &Path,
    raw: &str,
    location: &str,
) -> RecorderResult<PathBuf> {
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

pub(crate) fn normalize_glob_pattern(pattern: &str, project_root: &Path) -> RecorderResult<String> {
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

pub(crate) fn pathbuf_to_posix(path: &Path) -> String {
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
pub struct RawFilterFile {
    pub meta: RawMeta,
    #[serde(default)]
    pub io: Option<RawIo>,
    pub scope: RawScope,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawMeta {
    pub name: String,
    pub version: u32,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawIo {
    #[serde(default)]
    pub capture: Option<bool>,
    #[serde(default)]
    pub streams: Option<Vec<String>>,
    #[serde(default)]
    pub modes: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawScope {
    pub default_exec: String,
    pub default_value_action: String,
    #[serde(default)]
    pub rules: Option<Vec<RawScopeRule>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawScopeRule {
    pub selector: String,
    #[serde(default)]
    pub exec: Option<String>,
    #[serde(default)]
    pub value_default: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub value_patterns: Option<Vec<RawValuePattern>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawValuePattern {
    pub selector: String,
    pub action: String,
    #[serde(default)]
    pub reason: Option<String>,
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

fn rebuild_selector(
    kind: SelectorKind,
    match_type: MatchType,
    pattern: &str,
) -> RecorderResult<Selector> {
    let raw = match match_type {
        MatchType::Glob => format!("{}:{}", kind.token(), pattern),
        MatchType::Regex => format!("{}:regex:{}", kind.token(), pattern),
        MatchType::Literal => format!("{}:literal:{}", kind.token(), pattern),
    };
    Selector::parse(&raw, &[kind])
}
