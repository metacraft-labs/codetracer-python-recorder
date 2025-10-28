//! Runtime filter engine evaluating scope selectors and value policies for code objects.
//!
//! The engine consumes a [`TraceFilterConfig`](crate::trace_filter::config::TraceFilterConfig)
//! and caches per-code-object resolutions so the hot tracing callbacks only pay a fast lookup.

use crate::code_object::CodeObjectWrapper;
use crate::trace_filter::config::{
    ExecDirective, FilterSource, FilterSummary, ScopeRule, TraceFilterConfig, ValueAction,
    ValuePattern,
};
use crate::trace_filter::selector::{Selector, SelectorKind};
use dashmap::DashMap;
use pyo3::{prelude::*, PyErr};
use recorder_errors::{target, ErrorCode, RecorderResult};
use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

/// Final execution decision emitted by the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecDecision {
    Trace,
    Skip,
}

impl From<ExecDirective> for ExecDecision {
    fn from(value: ExecDirective) -> Self {
        match value {
            ExecDirective::Trace => ExecDecision::Trace,
            ExecDirective::Skip => ExecDecision::Skip,
        }
    }
}

/// Kind of value inspected while deciding redaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    Local,
    Global,
    Arg,
    Return,
    Attr,
}

impl ValueKind {
    fn selector_kind(self) -> SelectorKind {
        match self {
            ValueKind::Local => SelectorKind::Local,
            ValueKind::Global => SelectorKind::Global,
            ValueKind::Arg => SelectorKind::Arg,
            ValueKind::Return => SelectorKind::Return,
            ValueKind::Attr => SelectorKind::Attr,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ValueKind::Local => "local",
            ValueKind::Global => "global",
            ValueKind::Arg => "argument",
            ValueKind::Return => "return",
            ValueKind::Attr => "attribute",
        }
    }

    pub fn index(self) -> usize {
        match self {
            ValueKind::Local => 0,
            ValueKind::Global => 1,
            ValueKind::Arg => 2,
            ValueKind::Return => 3,
            ValueKind::Attr => 4,
        }
    }

    pub const ALL: [ValueKind; 5] = [
        ValueKind::Local,
        ValueKind::Global,
        ValueKind::Arg,
        ValueKind::Return,
        ValueKind::Attr,
    ];
}

/// Value redaction strategy resolved for a scope.
#[derive(Debug, Clone)]
pub struct ValuePolicy {
    default_action: ValueAction,
    patterns: Arc<[CompiledValuePattern]>,
}

impl ValuePolicy {
    fn new(default_action: ValueAction, patterns: Arc<[CompiledValuePattern]>) -> Self {
        ValuePolicy {
            default_action,
            patterns,
        }
    }

    /// Default action applied when no selector matches.
    pub fn default_action(&self) -> ValueAction {
        self.default_action
    }

    /// Evaluate the policy for a value of `kind` with identifier `name`.
    pub fn decide(&self, kind: ValueKind, name: &str) -> ValueAction {
        let selector_kind = kind.selector_kind();
        for pattern in self.patterns.iter() {
            if pattern.selector.kind() == selector_kind && pattern.selector.matches(name) {
                return pattern.action;
            }
        }
        self.default_action
    }

    /// Expose rule metadata for debugging or telemetry.
    pub fn patterns(&self) -> &[CompiledValuePattern] {
        &self.patterns
    }
}

/// Resolution emitted by the engine for a given code object.
#[derive(Debug, Clone)]
pub struct ScopeResolution {
    exec: ExecDecision,
    value_policy: Arc<ValuePolicy>,
    module_name: Option<String>,
    object_name: Option<String>,
    relative_path: Option<String>,
    absolute_path: Option<String>,
    matched_rule_index: Option<usize>,
    matched_rule_source: Option<usize>,
    matched_rule_reason: Option<String>,
}

impl ScopeResolution {
    /// Execution decision (trace vs skip).
    pub fn exec(&self) -> ExecDecision {
        self.exec
    }

    /// Value redaction policy derived for this scope.
    pub fn value_policy(&self) -> &ValuePolicy {
        &self.value_policy
    }

    /// Module name derived from the code object's filename (if any).
    pub fn module_name(&self) -> Option<&str> {
        self.module_name.as_deref()
    }

    /// Qualified object identifier (module + qualname when available).
    pub fn object_name(&self) -> Option<&str> {
        self.object_name.as_deref()
    }

    /// Project-relative POSIX path for the file containing the code object.
    pub fn relative_path(&self) -> Option<&str> {
        self.relative_path.as_deref()
    }

    /// Absolute POSIX path for the file containing the code object.
    pub fn absolute_path(&self) -> Option<&str> {
        self.absolute_path.as_deref()
    }

    /// Index within the flattened rule list that last matched this code object.
    pub fn matched_rule_index(&self) -> Option<usize> {
        self.matched_rule_index
    }

    /// Source identifier (filter file index) of the last matched rule.
    pub fn matched_rule_source(&self) -> Option<usize> {
        self.matched_rule_source
    }

    /// Reason string attached to the last matched rule, if present.
    pub fn matched_rule_reason(&self) -> Option<&str> {
        self.matched_rule_reason.as_deref()
    }
}

/// Runtime engine wrapping a compiled filter configuration.
pub struct TraceFilterEngine {
    config: Arc<TraceFilterConfig>,
    default_exec: ExecDecision,
    default_value_action: ValueAction,
    default_value_source: usize,
    rules: Arc<[CompiledScopeRule]>,
    cache: DashMap<usize, Arc<ScopeResolution>>,
}

impl TraceFilterEngine {
    /// Construct the engine from a fully resolved configuration.
    pub fn new(config: TraceFilterConfig) -> Self {
        let default_exec = config.default_exec().into();
        let default_value_action = config.default_value_action();
        let default_value_source = config.default_value_source();
        let rules = compile_rules(config.rules());

        TraceFilterEngine {
            config: Arc::new(config),
            default_exec,
            default_value_action,
            default_value_source,
            rules,
            cache: DashMap::new(),
        }
    }

    /// Resolve the scope decision for `code`, reusing cached results when available.
    pub fn resolve<'py>(
        &self,
        py: Python<'py>,
        code: &CodeObjectWrapper,
    ) -> RecorderResult<Arc<ScopeResolution>> {
        if let Some(entry) = self.cache.get(&code.id()) {
            return Ok(entry.clone());
        }

        let resolution = Arc::new(self.resolve_uncached(py, code)?);
        let entry = self
            .cache
            .entry(code.id())
            .or_insert_with(|| resolution.clone());
        Ok(entry.clone())
    }

    fn resolve_uncached(
        &self,
        py: Python<'_>,
        code: &CodeObjectWrapper,
    ) -> RecorderResult<ScopeResolution> {
        let filename = code
            .filename(py)
            .map_err(|err| py_attr_error("co_filename", err))?;
        let qualname = code
            .qualname(py)
            .map_err(|err| py_attr_error("co_qualname", err))?;

        let context = ScopeContext::derive(filename, qualname, self.config.sources());

        let mut exec = self.default_exec;
        let mut value_default = self.default_value_action;
        let mut value_default_source = self.default_value_source;
        let mut patterns: Arc<[CompiledValuePattern]> = Arc::from(Vec::new());
        let mut matched_rule_index = None;
        let mut matched_rule_source = context.source_id;
        let mut matched_rule_reason = None;

        for rule in self.rules.iter() {
            if rule.matches(&context) {
                if let Some(rule_exec) = rule.exec {
                    exec = rule_exec;
                }
                if let Some(rule_value) = rule.value_default {
                    value_default = rule_value;
                    value_default_source = rule.source_id;
                }
                patterns = rule.value_patterns.clone();
                matched_rule_index = Some(rule.index);
                matched_rule_source = Some(rule.source_id);
                matched_rule_reason = rule.reason.clone();
            }
        }

        let patterns = if value_default == ValueAction::Drop {
            if patterns
                .iter()
                .all(|pattern| pattern.source_id >= value_default_source)
            {
                patterns
            } else {
                let filtered: Vec<CompiledValuePattern> = patterns
                    .iter()
                    .filter(|pattern| pattern.source_id >= value_default_source)
                    .cloned()
                    .collect();
                filtered.into()
            }
        } else {
            patterns
        };

        let value_policy = Arc::new(ValuePolicy::new(value_default, patterns));

        Ok(ScopeResolution {
            exec,
            value_policy,
            module_name: context.module_name,
            object_name: context.object_name,
            relative_path: context.relative_path,
            absolute_path: context.absolute_path,
            matched_rule_index,
            matched_rule_source,
            matched_rule_reason,
        })
    }

    /// Return a summary of the filters that produced this engine.
    pub fn summary(&self) -> FilterSummary {
        self.config.summary()
    }
}

#[derive(Debug, Clone)]
struct CompiledScopeRule {
    selector: Selector,
    exec: Option<ExecDecision>,
    value_default: Option<ValueAction>,
    value_patterns: Arc<[CompiledValuePattern]>,
    reason: Option<String>,
    source_id: usize,
    index: usize,
}

impl CompiledScopeRule {
    fn matches(&self, context: &ScopeContext) -> bool {
        match self.selector.kind() {
            SelectorKind::Package => context
                .module_name
                .as_deref()
                .map(|module| self.selector.matches(module))
                .unwrap_or(false),
            SelectorKind::File => context
                .relative_path
                .as_deref()
                .map(|path| self.selector.matches(path))
                .or_else(|| {
                    context
                        .absolute_path
                        .as_deref()
                        .map(|path| self.selector.matches(path))
                })
                .unwrap_or(false),
            SelectorKind::Object => context
                .object_name
                .as_deref()
                .map(|object| self.selector.matches(object))
                .unwrap_or(false),
            _ => false,
        }
    }
}

/// A compiled value selector and its associated action.
#[derive(Debug, Clone)]
pub struct CompiledValuePattern {
    pub selector: Selector,
    pub action: ValueAction,
    pub reason: Option<String>,
    pub source_id: usize,
}

fn compile_rules(rules: &[ScopeRule]) -> Arc<[CompiledScopeRule]> {
    let compiled: Vec<CompiledScopeRule> = rules
        .iter()
        .enumerate()
        .map(|(index, rule)| CompiledScopeRule {
            selector: rule.selector.clone(),
            exec: rule.exec.map(ExecDecision::from),
            value_default: rule.value_default,
            value_patterns: compile_value_patterns(&rule.value_patterns),
            reason: rule.reason.clone(),
            source_id: rule.source_id,
            index,
        })
        .collect();
    compiled.into()
}

fn compile_value_patterns(patterns: &[ValuePattern]) -> Arc<[CompiledValuePattern]> {
    let compiled: Vec<CompiledValuePattern> = patterns
        .iter()
        .map(|pattern| CompiledValuePattern {
            selector: pattern.selector.clone(),
            action: pattern.action,
            reason: pattern.reason.clone(),
            source_id: pattern.source_id,
        })
        .collect();
    compiled.into()
}

#[derive(Debug)]
struct ScopeContext {
    module_name: Option<String>,
    object_name: Option<String>,
    relative_path: Option<String>,
    absolute_path: Option<String>,
    source_id: Option<usize>,
}

impl ScopeContext {
    fn derive(filename: &str, qualname: &str, sources: &[FilterSource]) -> Self {
        let absolute_path = normalise_to_posix(Path::new(filename));

        let mut best_match: Option<(usize, PathBuf)> = None;
        for (idx, source) in sources.iter().enumerate() {
            if let Ok(stripped) = Path::new(filename).strip_prefix(&source.project_root) {
                let stripped_owned = stripped.to_path_buf();
                let better = match &best_match {
                    Some((_, current)) => {
                        stripped_owned.components().count() >= current.components().count()
                    }
                    None => true,
                };
                if better {
                    best_match = Some((idx, stripped_owned));
                }
            }
        }

        let (source_id, relative_path) = best_match.map_or((None, None), |(idx, rel)| {
            let normalized = normalise_relative(rel);
            if normalized.is_empty() {
                (Some(idx), None)
            } else {
                (Some(idx), Some(normalized))
            }
        });

        let module_name = relative_path
            .as_deref()
            .and_then(|rel| module_from_relative(rel).map(|cow| cow.into_owned()));

        let object_name = module_name
            .as_ref()
            .map(|module| format!("{}.{}", module, qualname))
            .or_else(|| {
                if qualname.is_empty() {
                    None
                } else {
                    Some(qualname.to_string())
                }
            });

        ScopeContext {
            module_name,
            object_name,
            relative_path,
            absolute_path,
            source_id,
        }
    }
}

fn normalise_to_posix(path: &Path) -> Option<String> {
    if path.as_os_str().is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy()),
            Component::Prefix(prefix) => parts.push(prefix.as_os_str().to_string_lossy()),
            Component::RootDir => parts.push(Cow::Borrowed("")),
            Component::CurDir => continue,
            Component::ParentDir => {
                parts.push(Cow::Borrowed(".."));
            }
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

fn normalise_relative(relative: PathBuf) -> String {
    let mut components = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => components.push(part.to_string_lossy().to_string()),
            Component::CurDir => continue,
            Component::ParentDir => {
                if !components.is_empty() {
                    components.pop();
                }
            }
            _ => {}
        }
    }
    components.join("/")
}

fn module_from_relative(relative: &str) -> Option<Cow<'_, str>> {
    if relative.is_empty() {
        return None;
    }
    let trimmed = relative.trim_start_matches("./");
    let without_suffix = trimmed.strip_suffix(".py").unwrap_or(trimmed);
    if without_suffix.is_empty() {
        return None;
    }
    let mut parts: Vec<&str> = without_suffix.split('/').collect();
    if let Some(last) = parts.last().copied() {
        if last == "__init__" {
            parts.pop();
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(Cow::Owned(parts.join(".")))
}

fn py_attr_error(attr: &str, err: PyErr) -> recorder_errors::RecorderError {
    target!(
        ErrorCode::FrameIntrospectionFailed,
        "failed to read {} from code object: {}",
        attr,
        err
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace_filter::config::TraceFilterConfig;
    use pyo3::types::{PyAny, PyCode, PyModule};
    use std::ffi::CString;
    use std::fs;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn caches_resolution_and_applies_value_patterns() -> RecorderResult<()> {
        let (config, file_path) = filter_with_pkg_rule(
            r#"
            [scope]
            default_exec = "skip"
            default_value_action = "redact"

            [[scope.rules]]
            selector = "pkg:app.foo"
            exec = "trace"
            value_default = "allow"

            [[scope.rules.value_patterns]]
            selector = "local:literal:user"
            action = "allow"

            [[scope.rules.value_patterns]]
            selector = "arg:password"
            action = "redact"

            [[scope.rules.value_patterns]]
            selector = "local:temp"
            action = "drop"
            "#,
        )?;

        Python::with_gil(|py| -> RecorderResult<()> {
            let module = load_module(
                py,
                "app.foo",
                &file_path,
                "def foo(user, password):\n    return user\n",
            )?;
            let code_obj = get_code(&module, "foo")?;
            let wrapper = CodeObjectWrapper::new(py, &code_obj);

            let engine = TraceFilterEngine::new(config);

            let first = engine.resolve(py, &wrapper)?;
            assert_eq!(first.exec(), ExecDecision::Trace);
            assert_eq!(first.matched_rule_index(), Some(0));
            assert_eq!(first.module_name(), Some("app.foo"));
            assert_eq!(first.relative_path(), Some("app/foo.py"));

            let policy = first.value_policy();
            assert_eq!(policy.default_action(), ValueAction::Allow);
            assert_eq!(policy.decide(ValueKind::Local, "user"), ValueAction::Allow);
            assert_eq!(
                policy.decide(ValueKind::Arg, "password"),
                ValueAction::Redact
            );
            assert_eq!(policy.decide(ValueKind::Local, "temp"), ValueAction::Drop);
            assert_eq!(
                policy.decide(ValueKind::Global, "anything"),
                ValueAction::Allow
            );

            let second = engine.resolve(py, &wrapper)?;
            assert!(Arc::ptr_eq(&first, &second));
            Ok(())
        })
    }

    #[test]
    fn object_rule_overrides_package_rule() -> RecorderResult<()> {
        let (config, file_path) = filter_with_pkg_rule(
            r#"
            [scope]
            default_exec = "trace"
            default_value_action = "allow"

            [[scope.rules]]
            selector = "pkg:app.foo"
            exec = "skip"

            [[scope.rules]]
            selector = "obj:app.foo.bar"
            exec = "trace"
            value_default = "redact"
            "#,
        )?;

        Python::with_gil(|py| -> RecorderResult<()> {
            let module = load_module(
                py,
                "app.foo",
                &file_path,
                "def bar():\n    secret = 1\n    return secret\n",
            )?;
            let code_obj = get_code(&module, "bar")?;
            let wrapper = CodeObjectWrapper::new(py, &code_obj);

            let engine = TraceFilterEngine::new(config);
            let resolution = engine.resolve(py, &wrapper)?;

            assert_eq!(resolution.exec(), ExecDecision::Trace);
            assert_eq!(resolution.matched_rule_index(), Some(1));
            assert_eq!(
                resolution.value_policy().default_action(),
                ValueAction::Redact
            );
            Ok(())
        })
    }

    #[test]
    fn file_selector_matches_relative_path() -> RecorderResult<()> {
        let (config, file_path) = filter_with_pkg_rule(
            r#"
            [scope]
            default_exec = "trace"
            default_value_action = "allow"

            [[scope.rules]]
            selector = "file:app/foo.py"
            exec = "skip"
            "#,
        )?;

        Python::with_gil(|py| -> RecorderResult<()> {
            let module = load_module(py, "app.foo", &file_path, "def baz():\n    return 42\n")?;
            let code_obj = get_code(&module, "baz")?;
            let wrapper = CodeObjectWrapper::new(py, &code_obj);

            let engine = TraceFilterEngine::new(config);
            let resolution = engine.resolve(py, &wrapper)?;

            assert_eq!(resolution.exec(), ExecDecision::Skip);
            assert_eq!(resolution.relative_path(), Some("app/foo.py"));
            Ok(())
        })
    }

    fn filter_with_pkg_rule(body: &str) -> RecorderResult<(TraceFilterConfig, String)> {
        let temp = tempdir().expect("temp dir");
        let project_root = temp.path();
        let codetracer_dir = project_root.join(".codetracer");
        fs::create_dir(&codetracer_dir).unwrap();

        let filter_path = codetracer_dir.join("filters.toml");
        write_filter(&filter_path, body);

        let config = TraceFilterConfig::from_paths(&[filter_path])?;

        let file_path = project_root.join("app").join("foo.py");
        fs::create_dir_all(file_path.parent().unwrap()).unwrap();
        // Touch the file so the path exists for debugging.
        fs::File::create(&file_path).unwrap();

        Ok((config, file_path.to_string_lossy().to_string()))
    }

    fn write_filter(path: &Path, body: &str) {
        let mut file = fs::File::create(path).unwrap();
        writeln!(
            file,
            r#"
            [meta]
            name = "test"
            version = 1

            {}
            "#,
            body.trim()
        )
        .unwrap();
    }

    fn load_module<'py>(
        py: Python<'py>,
        module_name: &str,
        file_path: &str,
        source: &str,
    ) -> RecorderResult<Bound<'py, PyModule>> {
        let code_c = CString::new(source).expect("source without NUL");
        let file_c = CString::new(file_path).expect("path without NUL");
        let module_c = CString::new(module_name).expect("module without NUL");

        let module = PyModule::from_code(
            py,
            code_c.as_c_str(),
            file_c.as_c_str(),
            module_c.as_c_str(),
        )
        .map_err(|err| {
            target!(
                ErrorCode::FrameIntrospectionFailed,
                "failed to load module for engine test: {}",
                err
            )
        })?;
        Ok(module.into())
    }

    fn get_code<'py>(
        module: &Bound<'py, PyModule>,
        func_name: &str,
    ) -> RecorderResult<Bound<'py, PyCode>> {
        let func: Bound<'py, PyAny> = module
            .getattr(func_name)
            .map_err(|err| py_attr_error("function", err))?;
        let code_obj = func
            .getattr("__code__")
            .map_err(|err| py_attr_error("__code__", err))?
            .downcast_into::<PyCode>()
            .map_err(|err| py_attr_error("__code__", err.into()))?;
        Ok(code_obj)
    }
}
