use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::errors::Result;
use crate::trace_filter::config::TraceFilterConfig;
use crate::trace_filter::engine::TraceFilterEngine;

use super::filesystem::resolve_program_directory;

const TRACE_FILTER_DIR: &str = ".codetracer";
const TRACE_FILTER_FILE: &str = "trace-filter.toml";
const BUILTIN_FILTER_LABEL: &str = "builtin-default";
const BUILTIN_TRACE_FILTER: &str =
    include_str!("../../../resources/trace_filters/builtin_default.toml");

// Framework-specific builtin filters
const BUILTIN_PYTEST_FILTER_LABEL: &str = "builtin-pytest";
const BUILTIN_PYTEST_FILTER: &str =
    include_str!("../../../resources/trace_filters/builtin_pytest.toml");

const BUILTIN_UNITTEST_FILTER_LABEL: &str = "builtin-unittest";
const BUILTIN_UNITTEST_FILTER: &str =
    include_str!("../../../resources/trace_filters/builtin_unittest.toml");

pub fn load_trace_filter(
    explicit: Option<&[PathBuf]>,
    program: &str,
) -> Result<Option<Arc<TraceFilterEngine>>> {
    load_trace_filter_with_framework(explicit, program, None)
}

pub fn load_trace_filter_with_framework(
    explicit: Option<&[PathBuf]>,
    program: &str,
    test_framework: Option<&str>,
) -> Result<Option<Arc<TraceFilterEngine>>> {
    let mut chain: Vec<PathBuf> = Vec::new();

    if let Some(default) = discover_default_trace_filter(program)? {
        chain.push(default);
    }

    if let Some(paths) = explicit {
        chain.extend(paths.iter().cloned());
    }

    // Build the list of inline filters - builtin-default is always included
    let mut inline_filters: Vec<(&str, &str)> = vec![(BUILTIN_FILTER_LABEL, BUILTIN_TRACE_FILTER)];

    // Add framework-specific filter if requested
    if let Some(framework) = test_framework {
        match framework {
            "pytest" => {
                inline_filters.push((BUILTIN_PYTEST_FILTER_LABEL, BUILTIN_PYTEST_FILTER));
            }
            "unittest" => {
                inline_filters.push((BUILTIN_UNITTEST_FILTER_LABEL, BUILTIN_UNITTEST_FILTER));
            }
            _ => {
                // Unknown framework - log warning but continue
                log::warn!("Unknown test framework '{}', no builtin filter applied", framework);
            }
        }
    }

    let config = TraceFilterConfig::from_inline_and_paths(&inline_filters, &chain)?;
    Ok(Some(Arc::new(TraceFilterEngine::new(config))))
}

fn discover_default_trace_filter(program: &str) -> Result<Option<PathBuf>> {
    let start_dir = resolve_program_directory(program)?;
    let mut current: Option<&Path> = Some(start_dir.as_path());
    while let Some(dir) = current {
        let candidate = dir.join(TRACE_FILTER_DIR).join(TRACE_FILTER_FILE);
        if matches!(std::fs::metadata(&candidate), Ok(metadata) if metadata.is_file()) {
            return Ok(Some(candidate));
        }
        current = dir.parent();
    }
    Ok(None)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    pub fn write_default_filter(root: &Path) -> PathBuf {
        let filters_dir = root.join(TRACE_FILTER_DIR);
        fs::create_dir_all(&filters_dir).expect("create filter dir");
        let filter_path = filters_dir.join(TRACE_FILTER_FILE);
        fs::write(
            &filter_path,
            r#"
            [meta]
            name = "default"
            version = 1

            [scope]
            default_exec = "trace"
            default_value_action = "allow"

            [[scope.rules]]
            selector = "pkg:src"
            exec = "trace"
            value_default = "allow"
            "#,
        )
        .expect("write filter");
        filter_path
    }

    pub fn write_app(root: &Path) -> PathBuf {
        let app_dir = root.join("src");
        fs::create_dir_all(&app_dir).expect("create src dir");
        let script_path = app_dir.join("main.py");
        fs::write(&script_path, "print('run')\n").expect("write script");
        script_path
    }

    pub fn write_default_and_override(root: &Path) -> (PathBuf, PathBuf) {
        let default = write_default_filter(root);
        let override_filter_path = root.join("override-filter.toml");
        fs::write(
            &override_filter_path,
            r#"
            [meta]
            name = "override"
            version = 1

            [scope]
            default_exec = "trace"
            default_value_action = "allow"

            [[scope.rules]]
            selector = "pkg:src.special"
            exec = "skip"
            value_default = "redact"
            "#,
        )
        .expect("write override filter");
        (default, override_filter_path)
    }

    #[test]
    fn discover_filter_walks_directories() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();
        write_default_filter(root);
        let script = write_app(root);
        let found =
            discover_default_trace_filter(script.to_str().expect("utf8")).expect("discover");
        assert!(found.is_some());
    }

    #[test]
    fn load_trace_filter_includes_builtin() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();
        let script = write_app(root);

        let engine = load_trace_filter(None, script.to_str().expect("utf8"))
            .expect("load")
            .expect("engine");
        let summary = engine.summary();
        assert!(summary
            .entries
            .iter()
            .any(|entry| entry.path == PathBuf::from("<inline:builtin-default>")));
    }

    #[test]
    fn load_trace_filter_merges_default_and_override() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();
        let script = write_app(root);
        let (default_filter_path, override_filter_path) = write_default_and_override(root);

        let engine = load_trace_filter(
            Some(&[override_filter_path.clone()]),
            script.to_str().expect("utf8"),
        )
        .expect("load")
        .expect("engine");
        let paths: Vec<PathBuf> = engine
            .summary()
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        assert!(paths.contains(&PathBuf::from("<inline:builtin-default>")));
        assert!(paths.contains(&default_filter_path));
        assert!(paths.contains(&override_filter_path));
    }

    #[test]
    fn load_trace_filter_with_pytest_framework_includes_pytest_filter() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();
        let script = write_app(root);

        let engine = load_trace_filter_with_framework(
            None,
            script.to_str().expect("utf8"),
            Some("pytest"),
        )
        .expect("load")
        .expect("engine");

        let paths: Vec<PathBuf> = engine
            .summary()
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect();

        assert!(paths.contains(&PathBuf::from("<inline:builtin-default>")));
        assert!(paths.contains(&PathBuf::from("<inline:builtin-pytest>")));
    }

    #[test]
    fn load_trace_filter_with_unittest_framework_includes_unittest_filter() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();
        let script = write_app(root);

        let engine = load_trace_filter_with_framework(
            None,
            script.to_str().expect("utf8"),
            Some("unittest"),
        )
        .expect("load")
        .expect("engine");

        let paths: Vec<PathBuf> = engine
            .summary()
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect();

        assert!(paths.contains(&PathBuf::from("<inline:builtin-default>")));
        assert!(paths.contains(&PathBuf::from("<inline:builtin-unittest>")));
    }

    #[test]
    fn load_trace_filter_without_framework_excludes_framework_filters() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();
        let script = write_app(root);

        let engine = load_trace_filter_with_framework(
            None,
            script.to_str().expect("utf8"),
            None,
        )
        .expect("load")
        .expect("engine");

        let paths: Vec<PathBuf> = engine
            .summary()
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect();

        assert!(paths.contains(&PathBuf::from("<inline:builtin-default>")));
        assert!(!paths.contains(&PathBuf::from("<inline:builtin-pytest>")));
        assert!(!paths.contains(&PathBuf::from("<inline:builtin-unittest>")));
    }

    #[test]
    fn load_trace_filter_unknown_framework_only_includes_default() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();
        let script = write_app(root);

        let engine = load_trace_filter_with_framework(
            None,
            script.to_str().expect("utf8"),
            Some("unknown_framework"),
        )
        .expect("load")
        .expect("engine");

        let paths: Vec<PathBuf> = engine
            .summary()
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect();

        // Should still have builtin-default but no unknown framework filter
        assert!(paths.contains(&PathBuf::from("<inline:builtin-default>")));
        assert_eq!(paths.len(), 1, "Should only have builtin-default filter");
    }
}
