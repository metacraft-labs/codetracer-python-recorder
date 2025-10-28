//! Shared helpers for deriving Python module names from filenames and module metadata.

use std::borrow::Cow;
use std::path::{Component, Path};
use std::sync::Arc;

use dashmap::DashMap;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict, PyList};

use crate::code_object::CodeObjectWrapper;

/// Resolver that infers module names for absolute file paths using sys.path roots and
/// sys.modules fallbacks. Results are cached per path to avoid repeated scans.
#[derive(Debug, Clone)]
pub struct ModuleIdentityResolver {
    module_roots: Arc<[String]>,
    cache: DashMap<String, Option<String>>,
}

impl ModuleIdentityResolver {
    /// Construct a resolver using the current `sys.path`.
    pub fn new() -> Self {
        let roots = Python::with_gil(|py| collect_module_roots(py));
        Self::from_roots(roots)
    }

    /// Construct a resolver from an explicit list of module roots. Visible for tests.
    pub fn from_roots(roots: Vec<String>) -> Self {
        Self {
            module_roots: Arc::from(roots),
            cache: DashMap::new(),
        }
    }

    /// Resolve the module name for an absolute POSIX path (if any).
    pub fn resolve_absolute(&self, py: Python<'_>, absolute: &str) -> Option<String> {
        if let Some(entry) = self.cache.get(absolute) {
            return entry.clone();
        }
        let resolved = module_name_from_roots(self.module_roots(), absolute)
            .or_else(|| lookup_module_name(py, absolute));
        self.cache.insert(absolute.to_string(), resolved.clone());
        resolved
    }

    /// Expose the sys.path roots used by this resolver (primarily for tests).
    pub fn module_roots(&self) -> &[String] {
        &self.module_roots
    }
}

/// Caches module names per code object id, reusing a shared resolver for filesystem lookups.
#[allow(dead_code)]
#[derive(Debug)]
pub struct ModuleIdentityCache {
    resolver: ModuleIdentityResolver,
    code_cache: DashMap<usize, Option<String>>,
}

#[allow(dead_code)]
impl ModuleIdentityCache {
    /// Construct with a fresh resolver seeded from `sys.path`.
    pub fn new() -> Self {
        Self::with_resolver(ModuleIdentityResolver::new())
    }

    /// Construct using an explicit resolver (primarily for tests).
    pub fn with_resolver(resolver: ModuleIdentityResolver) -> Self {
        Self {
            resolver,
            code_cache: DashMap::new(),
        }
    }

    /// Resolve the dotted module name for a Python code object, caching the result by code id.
    pub fn resolve_for_code<'py>(
        &self,
        py: Python<'py>,
        code: &CodeObjectWrapper,
        hints: ModuleNameHints<'_>,
    ) -> Option<String> {
        if let Some(entry) = self.code_cache.get(&code.id()) {
            return entry.clone();
        }

        let mut resolved = hints
            .preferred
            .and_then(sanitise_module_name)
            .or_else(|| {
                hints
                    .relative_path
                    .and_then(|relative| module_from_relative(relative))
            })
            .or_else(|| {
                hints
                    .absolute_path
                    .and_then(|absolute| self.resolver.resolve_absolute(py, absolute))
            })
            .or_else(|| hints.globals_name.and_then(sanitise_module_name));

        if resolved.is_none() && hints.absolute_path.is_none() {
            if let Ok(filename) = code.filename(py) {
                let path = Path::new(filename);
                if path.is_absolute() {
                    if let Some(normalized) = normalise_to_posix(path) {
                        resolved = self.resolver.resolve_absolute(py, normalized.as_str());
                    }
                }
            }
        }

        self.code_cache.insert(code.id(), resolved.clone());
        resolved
    }

    /// Remove a cached module name for a code id.
    pub fn invalidate(&self, code_id: usize) {
        self.code_cache.remove(&code_id);
    }

    /// Clear all cached code-object mappings.
    pub fn clear(&self) {
        self.code_cache.clear();
    }

    /// Access the underlying resolver (primarily for tests/runtime wiring).
    pub fn resolver(&self) -> &ModuleIdentityResolver {
        &self.resolver
    }
}

/// Optional hints supplied when resolving module names.
#[allow(dead_code)]
#[derive(Debug, Default, Clone, Copy)]
pub struct ModuleNameHints<'a> {
    /// Module name provided by another subsystem (e.g., trace filters).
    pub preferred: Option<&'a str>,
    /// Normalised project-relative path (used for deterministic names within a project).
    pub relative_path: Option<&'a str>,
    /// Absolute POSIX path to the source file.
    pub absolute_path: Option<&'a str>,
    /// `__name__` extracted from frame globals during runtime tracing.
    pub globals_name: Option<&'a str>,
}

#[allow(dead_code)]
impl<'a> ModuleNameHints<'a> {
    pub fn new() -> Self {
        Self::default()
    }
}

fn collect_module_roots(py: Python<'_>) -> Vec<String> {
    let mut roots = Vec::new();
    if let Ok(sys) = py.import("sys") {
        if let Ok(path_obj) = sys.getattr("path") {
            if let Ok(path_list) = path_obj.downcast_into::<PyList>() {
                for entry in path_list.iter() {
                    if let Ok(raw) = entry.extract::<String>() {
                        if let Some(normalized) = normalise_to_posix(Path::new(&raw)) {
                            roots.push(normalized);
                        }
                    }
                }
            }
        }
    }
    roots
}

pub(crate) fn module_name_from_roots(roots: &[String], absolute: &str) -> Option<String> {
    for base in roots {
        if let Some(relative) = strip_posix_prefix(absolute, base) {
            if let Some(name) = relative_str_to_module(relative) {
                return Some(name);
            }
        }
    }
    None
}

fn lookup_module_name(py: Python<'_>, absolute: &str) -> Option<String> {
    let sys = py.import("sys").ok()?;
    let modules_obj = sys.getattr("modules").ok()?;
    let modules: Bound<'_, PyDict> = modules_obj.downcast_into::<PyDict>().ok()?;

    let mut best: Option<(usize, String)> = None;
    'modules: for (name_obj, module_obj) in modules.iter() {
        let module_name: String = name_obj.extract().ok()?;
        if module_obj.is_none() {
            continue;
        }
        for candidate in module_candidate_paths(&module_obj) {
            if equivalent_posix_paths(&candidate, absolute) {
                let preferred = preferred_module_name(&module_name, &module_obj);
                let score = module_name_score(&preferred);
                let update = match best {
                    Some((best_score, _)) => score < best_score,
                    None => true,
                };
                if update {
                    best = Some((score, preferred));
                    if score == 0 {
                        break 'modules;
                    }
                }
            }
        }
    }

    best.map(|(_, name)| name)
}

fn module_candidate_paths(module: &Bound<'_, PyAny>) -> Vec<String> {
    let mut candidates = Vec::new();
    if let Ok(spec) = module.getattr("__spec__") {
        if let Some(origin) = extract_normalised_spec_origin(&spec) {
            candidates.push(origin);
        }
    }
    if let Some(file) = extract_normalised_attr(module, "__file__") {
        candidates.push(file);
    }
    if let Some(cached) = extract_normalised_attr(module, "__cached__") {
        candidates.push(cached);
    }
    candidates
}

fn extract_normalised_attr(module: &Bound<'_, PyAny>, attr: &str) -> Option<String> {
    let value = module.getattr(attr).ok()?;
    extract_normalised_path(&value)
}

fn extract_normalised_spec_origin(spec: &Bound<'_, PyAny>) -> Option<String> {
    if spec.is_none() {
        return None;
    }
    let origin = spec.getattr("origin").ok()?;
    extract_normalised_path(&origin)
}

fn extract_normalised_path(value: &Bound<'_, PyAny>) -> Option<String> {
    if value.is_none() {
        return None;
    }
    let raw: String = value.extract().ok()?;
    normalise_to_posix(Path::new(raw.as_str()))
}

fn equivalent_posix_paths(candidate: &str, target: &str) -> bool {
    if candidate == target {
        return true;
    }
    if candidate.ends_with(".pyc") && target.ends_with(".py") {
        return candidate.trim_end_matches('c') == target;
    }
    false
}

fn preferred_module_name(default: &str, module: &Bound<'_, PyAny>) -> String {
    if let Ok(spec) = module.getattr("__spec__") {
        if let Ok(name) = spec.getattr("name") {
            if let Ok(raw) = name.extract::<String>() {
                if !raw.is_empty() {
                    return raw;
                }
            }
        }
    }
    if let Ok(name_attr) = module.getattr("__name__") {
        if let Ok(raw) = name_attr.extract::<String>() {
            if !raw.is_empty() {
                return raw;
            }
        }
    }
    default.to_string()
}

fn module_name_score(name: &str) -> usize {
    if name
        .split('.')
        .all(|segment| !segment.is_empty() && segment.chars().all(is_identifier_char))
    {
        0
    } else {
        1
    }
}

fn is_identifier_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

/// Convert a normalised relative path (e.g., `pkg/foo.py`) into a dotted module name.
pub fn module_from_relative(relative: &str) -> Option<String> {
    relative_str_to_module(relative)
}

#[allow(dead_code)]
fn sanitise_module_name(candidate: &str) -> Option<String> {
    let trimmed = candidate.trim();
    if trimmed.is_empty() {
        return None;
    }
    if is_valid_module_name(trimmed) {
        Some(trimmed.to_string())
    } else {
        None
    }
}

/// Return true when the supplied module name is a dotted identifier.
pub fn is_valid_module_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .split('.')
            .all(|segment| !segment.is_empty() && segment.chars().all(is_identifier_char))
}

fn relative_str_to_module(relative: &str) -> Option<String> {
    let mut parts: Vec<&str> = relative
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    if parts.is_empty() {
        return None;
    }
    let last = parts.pop().expect("non-empty");
    if let Some(stem) = last.strip_suffix(".py") {
        if stem != "__init__" {
            parts.push(stem);
        }
    } else {
        parts.push(last);
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("."))
}

fn strip_posix_prefix<'a>(path: &'a str, base: &str) -> Option<&'a str> {
    if base.is_empty() {
        return None;
    }
    if base == "/" {
        return path.strip_prefix('/');
    }
    if path.starts_with(base) {
        let mut remainder = &path[base.len()..];
        if remainder.starts_with('/') {
            remainder = &remainder[1..];
        }
        if remainder.is_empty() {
            None
        } else {
            Some(remainder)
        }
    } else {
        None
    }
}

/// Normalise a filesystem path to a POSIX-style string used by trace filters.
pub fn normalise_to_posix(path: &Path) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code_object::CodeObjectWrapper;
    use pyo3::types::{PyAny, PyCode, PyModule};
    use std::ffi::CString;

    fn load_module<'py>(
        py: Python<'py>,
        module_name: &str,
        file_path: &str,
        source: &str,
    ) -> PyResult<Bound<'py, PyModule>> {
        let code_c = CString::new(source).expect("source without NUL");
        let file_c = CString::new(file_path).expect("path without NUL");
        let module_c = CString::new(module_name).expect("module without NUL");
        PyModule::from_code(
            py,
            code_c.as_c_str(),
            file_c.as_c_str(),
            module_c.as_c_str(),
        )
    }

    fn get_code<'py>(module: &Bound<'py, PyModule>, func_name: &str) -> Bound<'py, PyCode> {
        let func: Bound<'py, PyAny> = module.getattr(func_name).expect("function");
        func.getattr("__code__")
            .expect("__code__ attr")
            .downcast_into::<PyCode>()
            .expect("PyCode")
    }

    #[test]
    fn normalise_to_posix_handles_common_paths() {
        let path = Path::new("src/lib/foo.py");
        assert_eq!(normalise_to_posix(path).as_deref(), Some("src/lib/foo.py"));
    }

    #[test]
    fn module_identity_cache_prefers_preferred_hint() {
        Python::with_gil(|py| {
            let module =
                load_module(py, "tmp_mod", "tmp_mod.py", "def foo():\n    return 1\n").unwrap();
            let code = get_code(&module, "foo");
            let wrapper = CodeObjectWrapper::new(py, &code);
            let cache = ModuleIdentityCache::new();
            let hints = ModuleNameHints {
                preferred: Some("pkg.actual"),
                ..ModuleNameHints::default()
            };
            let resolved = cache.resolve_for_code(py, &wrapper, hints);
            assert_eq!(resolved.as_deref(), Some("pkg.actual"));
        });
    }

    #[test]
    fn module_identity_cache_uses_resolver_for_absolute_paths() {
        Python::with_gil(|py| {
            let tmp = tempfile::tempdir().expect("tempdir");
            let module_path = tmp.path().join("pkg").join("mod.py");
            std::fs::create_dir_all(module_path.parent().unwrap()).expect("mkdir");
            std::fs::write(&module_path, "def foo():\n    return 1\n").expect("write source");

            let module_path_str = module_path.to_string_lossy().to_string();
            let module = load_module(
                py,
                "pkg.mod",
                module_path_str.as_str(),
                "def foo():\n    return 1\n",
            )
            .unwrap();
            let code = get_code(&module, "foo");
            let wrapper = CodeObjectWrapper::new(py, &code);
            let root = normalise_to_posix(tmp.path()).expect("normalize root");
            let resolver = ModuleIdentityResolver::from_roots(vec![root]);
            let cache = ModuleIdentityCache::with_resolver(resolver);
            let absolute_norm = normalise_to_posix(module_path.as_path()).expect("normalize abs");
            let hints = ModuleNameHints {
                absolute_path: Some(absolute_norm.as_str()),
                ..ModuleNameHints::default()
            };

            let resolved = cache.resolve_for_code(py, &wrapper, hints);
            assert_eq!(resolved.as_deref(), Some("pkg.mod"));
        });
    }

    #[test]
    fn module_from_relative_strips_init() {
        assert_eq!(
            module_from_relative("pkg/module/__init__.py").as_deref(),
            Some("pkg.module")
        );
        assert_eq!(
            module_from_relative("pkg/module/sub.py").as_deref(),
            Some("pkg.module.sub")
        );
    }
}
