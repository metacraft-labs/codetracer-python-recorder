//! Minimal helpers for deriving Python module identifiers from filesystem metadata.

use std::borrow::Cow;
use std::env;
use std::path::{Component, Path};

use pyo3::prelude::*;
use pyo3::types::PyList;

/// Convert a normalised relative path (e.g., `pkg/foo.py`) into a dotted module name.
pub fn module_from_relative(relative: &str) -> Option<String> {
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
    let base = if base == "/" {
        String::from("")
    } else {
        base.to_string()
    };
    if path == base {
        return None;
    }
    if path.starts_with(&base) {
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

/// Attempt to infer a module name by traversing `__init__.py` packages containing `path`.
pub fn module_name_from_packages(path: &Path) -> Option<String> {
    let mut segments: Vec<String> = Vec::new();
    let mut current = path.parent();
    while let Some(dir) = current {
        if dir.join("__init__.py").exists() {
            if let Some(component) = dir.file_name().and_then(|s| s.to_str()) {
                if is_valid_module_name(component) {
                    segments.push(component.to_string());
                    current = dir.parent();
                    continue;
                }
            }
        }
        break;
    }
    segments.reverse();

    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        if stem != "__init__" && is_valid_module_name(stem) {
            segments.push(stem.to_string());
        }
    }

    if segments.is_empty() {
        None
    } else {
        Some(segments.join("."))
    }
}

/// Return true when the supplied module name is a dotted identifier.
pub fn is_valid_module_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .split('.')
            .all(|segment| !segment.is_empty() && segment.chars().all(is_identifier_char))
}

fn is_identifier_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
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
    use pyo3::Python;
    use std::fs;
    use tempfile::tempdir;

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

    #[test]
    fn module_name_from_packages_detects_package_hierarchy() {
        let tmp = tempdir().expect("tempdir");
        let pkg_dir = tmp.path().join("pkg").join("sub");
        fs::create_dir_all(&pkg_dir).expect("create dirs");
        fs::write(pkg_dir.join("__init__.py"), "# pkg\n").expect("write __init__");
        fs::write(tmp.path().join("pkg").join("__init__.py"), "# pkg\n").expect("write init");

        let module_path = pkg_dir.join("mod.py");
        fs::write(&module_path, "value = 1\n").expect("write module");

        let derived = module_name_from_packages(module_path.as_path());
        assert_eq!(derived.as_deref(), Some("pkg.sub.mod"));
    }

    #[test]
    fn module_name_from_packages_ignores_non_packages() {
        let tmp = tempdir().expect("tempdir");
        let module_path = tmp.path().join("script.py");
        fs::write(&module_path, "value = 1\n").expect("write module");

        assert_eq!(
            module_name_from_packages(module_path.as_path()).as_deref(),
            Some("script")
        );
    }

    #[test]
    fn module_name_from_sys_path_uses_roots() {
        Python::with_gil(|py| {
            let tmp = tempdir().expect("tempdir");
            let pkg_dir = tmp.path().join("pkg");
            fs::create_dir_all(&pkg_dir).expect("create pkg");
            let module_path = pkg_dir.join("mod.py");
            fs::write(&module_path, "value = 1\n").expect("write module");

            let sys = py.import("sys").expect("import sys");
            let sys_path = sys.getattr("path").expect("sys.path");
            sys_path
                .call_method1("insert", (0, tmp.path().to_string_lossy().as_ref()))
                .expect("insert tmp root");

            let derived = module_name_from_sys_path(py, module_path.as_path());
            assert_eq!(derived.as_deref(), Some("pkg.mod"));

            sys_path
                .call_method1("pop", (0,))
                .expect("restore sys.path");
        });
    }

    #[test]
    fn normalise_to_posix_handles_common_paths() {
        let path = Path::new("src/lib/foo.py");
        assert_eq!(normalise_to_posix(path).as_deref(), Some("src/lib/foo.py"));
    }
}
pub fn module_name_from_sys_path(py: Python<'_>, path: &Path) -> Option<String> {
    let absolute = normalise_to_posix(path)?;
    let sys = py.import("sys").ok()?;
    let sys_path_obj = sys.getattr("path").ok()?;
    let sys_path = sys_path_obj.downcast::<PyList>().ok()?;

    let cwd = env::current_dir()
        .ok()
        .and_then(|dir| normalise_to_posix(dir.as_path()));

    for entry in sys_path.iter() {
        let raw = entry.extract::<String>().ok()?;
        let root = if raw.is_empty() {
            match cwd.as_ref() {
                Some(current) => current.clone(),
                None => continue,
            }
        } else if let Some(norm) = normalise_to_posix(Path::new(&raw)) {
            norm
        } else {
            continue;
        };

        if let Some(remainder) = strip_posix_prefix(&absolute, &root) {
            if let Some(name) = module_from_relative(remainder) {
                return Some(name);
            }
        }
    }

    None
}
