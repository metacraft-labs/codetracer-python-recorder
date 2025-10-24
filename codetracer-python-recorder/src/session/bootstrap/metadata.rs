use pyo3::prelude::*;
use recorder_errors::{enverr, ErrorCode, RecorderError};

use crate::errors::Result;

/// Basic metadata about the currently running Python program.
#[derive(Debug, Clone)]
pub struct ProgramMetadata {
    pub program: String,
    pub args: Vec<String>,
}

fn metadata_error(err: pyo3::PyErr) -> RecorderError {
    enverr!(ErrorCode::Io, "failed to collect program metadata")
        .with_context("details", err.to_string())
}

/// Capture program name and arguments from `sys.argv` for metadata records.
pub fn collect_program_metadata(py: Python<'_>) -> Result<ProgramMetadata> {
    let sys = py.import("sys").map_err(metadata_error)?;
    let argv = sys.getattr("argv").map_err(metadata_error)?;

    let program = match argv.get_item(0) {
        Ok(obj) => obj
            .extract::<String>()
            .unwrap_or_else(|_| "<unknown>".to_string()),
        Err(_) => "<unknown>".to_string(),
    };

    let args = match argv.len() {
        Ok(len) if len > 1 => {
            let mut items = Vec::with_capacity(len.saturating_sub(1));
            for idx in 1..len {
                if let Ok(value) = argv.get_item(idx).and_then(|obj| obj.extract::<String>()) {
                    items.push(value);
                }
            }
            items
        }
        _ => Vec::new(),
    };

    Ok(ProgramMetadata { program, args })
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use pyo3::types::PyList;

    /// Helper struct for building argv lists in tests.
    pub struct ProgramArgs<'a> {
        items: Vec<&'a str>,
    }

    impl<'a> ProgramArgs<'a> {
        pub fn new<const N: usize>(items: [&'a str; N]) -> Self {
            Self {
                items: items.to_vec(),
            }
        }

        pub fn empty() -> Self {
            Self { items: Vec::new() }
        }
    }

    pub fn with_sys_argv<F, R>(py: Python<'_>, args: ProgramArgs<'_>, op: F) -> R
    where
        F: FnOnce() -> R,
    {
        let sys = py.import("sys").expect("import sys");
        let original = sys.getattr("argv").expect("argv").unbind();
        let argv = PyList::new(py, args.items).expect("argv");
        sys.setattr("argv", argv).expect("set argv");

        let result = op();

        sys.setattr("argv", original.bind(py))
            .expect("restore argv");
        result
    }

    #[test]
    fn collects_program_and_args() {
        Python::with_gil(|py| {
            let metadata = with_sys_argv(
                py,
                ProgramArgs::new(["/tmp/prog.py", "--flag", "value"]),
                || collect_program_metadata(py),
            )
            .expect("metadata");
            assert_eq!(metadata.program, "/tmp/prog.py");
            assert_eq!(
                metadata.args,
                vec!["--flag".to_string(), "value".to_string()]
            );
        });
    }

    #[test]
    fn defaults_to_unknown_program() {
        Python::with_gil(|py| {
            let metadata = with_sys_argv(py, ProgramArgs::empty(), || collect_program_metadata(py))
                .expect("metadata");
            assert_eq!(metadata.program, "<unknown>");
            assert!(metadata.args.is_empty());
        });
    }
}
