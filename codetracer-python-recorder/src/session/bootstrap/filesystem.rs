use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use recorder_errors::{enverr, usage, ErrorCode};
use runtime_tracing::TraceEventsFileFormat;

use crate::errors::Result;

/// Ensure the requested trace directory exists and is writable.
pub fn ensure_trace_directory(path: &Path) -> Result<()> {
    if path.exists() {
        if !path.is_dir() {
            return Err(usage!(
                ErrorCode::TraceDirectoryConflict,
                "trace path exists and is not a directory"
            )
            .with_context("path", path.display().to_string()));
        }
        return Ok(());
    }

    fs::create_dir_all(path).map_err(|e| {
        enverr!(
            ErrorCode::TraceDirectoryCreateFailed,
            "failed to create trace directory"
        )
        .with_context("path", path.display().to_string())
        .with_context("io", e.to_string())
    })
}

/// Convert a user-provided format string into the runtime representation.
pub fn resolve_trace_format(value: &str) -> Result<TraceEventsFileFormat> {
    match value.to_ascii_lowercase().as_str() {
        "json" => Ok(TraceEventsFileFormat::Json),
        // Accept historical aliases for the binary format.
        "binary" | "binaryv0" | "binary_v0" | "b0" => Ok(TraceEventsFileFormat::BinaryV0),
        other => Err(usage!(
            ErrorCode::UnsupportedFormat,
            "unsupported trace format '{}'. Expected one of: json, binary",
            other
        )),
    }
}

pub fn resolve_program_directory(program: &str) -> Result<PathBuf> {
    let trimmed = program.trim();
    if trimmed.is_empty() || trimmed == "<unknown>" {
        return current_directory();
    }

    let path = Path::new(trimmed);
    if path.is_absolute() {
        if path.is_dir() {
            return Ok(path.to_path_buf());
        }
        if let Some(parent) = path.parent() {
            return Ok(parent.to_path_buf());
        }
        return current_directory();
    }

    let cwd = current_directory()?;
    let joined = cwd.join(path);
    if joined.is_dir() {
        return Ok(joined);
    }
    if let Some(parent) = joined.parent() {
        return Ok(parent.to_path_buf());
    }
    Ok(cwd)
}

pub fn current_directory() -> Result<PathBuf> {
    env::current_dir().map_err(|err| {
        enverr!(ErrorCode::Io, "failed to resolve current directory")
            .with_context("io", err.to_string())
    })
}
