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

pub fn load_trace_filter(
    explicit: Option<&[PathBuf]>,
    program: &str,
) -> Result<Option<Arc<TraceFilterEngine>>> {
    let mut chain: Vec<PathBuf> = Vec::new();

    if let Some(default) = discover_default_trace_filter(program)? {
        chain.push(default);
    }

    if let Some(paths) = explicit {
        chain.extend(paths.iter().cloned());
    }

    let config = TraceFilterConfig::from_inline_and_paths(
        &[(BUILTIN_FILTER_LABEL, BUILTIN_TRACE_FILTER)],
        &chain,
    )?;
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
    use std::path::Path;
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
}
