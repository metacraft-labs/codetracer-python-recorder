//! Per-file `paths.dat` Layout A tables for a column-aware trace.
//!
//! The writer fixes a file's table when the file is first interned, by an
//! explicit registration or by any step, function, call or id request that
//! names it, and refuses a different table later (`internal-files.md`
//! §"`paths.dat` Layout A"). Every path the recorder hands the writer
//! therefore goes through [`PathTables`], which registers the file's real
//! table before anything else can mention it.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use codetracer_trace_types::PathId;
use codetracer_trace_writer_nim::trace_writer::TraceWriter;

use crate::runtime::output_paths::{source_line_table, CONVENTIONAL_LINE_POSITIONS};
use crate::runtime::tracer::events::maybe_register_autoformat_view;

/// The files whose tables this trace has registered.
#[derive(Debug, Default)]
pub(crate) struct PathTables {
    column_aware: bool,
    registered: HashSet<PathBuf>,
    /// Files registered with the conventional table because their source
    /// could not be read; columns on them are clamped.
    conventional: HashSet<PathBuf>,
}

impl PathTables {
    pub(crate) fn new(column_aware: bool) -> Self {
        Self {
            column_aware,
            ..Self::default()
        }
    }

    /// Register `path`'s table, once, and run the one-shot autoformat pass
    /// on it. A no-op outside a column-aware trace and for a path already
    /// registered.
    pub(crate) fn register(&mut self, writer: &mut dyn TraceWriter, path: &Path) {
        if !self.column_aware || self.registered.contains(path) {
            return;
        }
        self.registered.insert(path.to_path_buf());
        let table = source_line_table(path);
        if table.conventional {
            self.conventional.insert(path.to_path_buf());
        }
        match TraceWriter::register_path_with_line_lengths(writer, path, &table.line_lengths) {
            Ok(_) => {
                // The id `register_path_with_line_lengths` returns is not
                // the path's; look the now-interned path up.
                let path_id = TraceWriter::ensure_path_id(writer, path);
                maybe_register_autoformat_view(writer, path_id, path);
            }
            Err(err) => {
                log::warn!(
                    "[PathTables] register_path_with_line_lengths failed for {}: {}",
                    path.display(),
                    err,
                );
            }
        }
    }

    /// Intern `path`, registering its table first, and return its id.
    pub(crate) fn intern(&mut self, writer: &mut dyn TraceWriter, path: &Path) -> PathId {
        self.register(writer, path);
        TraceWriter::ensure_path_id(writer, path)
    }

    /// The column a step on `path` is recorded at. A file registered with
    /// the conventional table has `CONVENTIONAL_LINE_POSITIONS` positions
    /// per line, so a column past that is recorded at the last one.
    pub(crate) fn step_column(&self, path: &Path, column: i64) -> i64 {
        if self.conventional.contains(path) {
            column.min(i64::from(CONVENTIONAL_LINE_POSITIONS))
        } else {
            column
        }
    }
}

/// `path` as an absolute, lexically normalized path, the form Python gives
/// a script's code (`os.path.abspath`): relative to the current directory,
/// with `.` and `..` folded. A program named by a relative `argv[0]` and
/// the filename of its code are then one string, so one `paths.dat` record.
pub(crate) fn absolute_program_path(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::absolute_program_path;
    use std::path::Path;

    #[test]
    fn a_relative_program_path_is_made_absolute_and_folded() {
        let cwd = std::env::current_dir().expect("cwd");
        assert_eq!(
            absolute_program_path(Path::new("a/./b/../c.py")),
            cwd.join("a").join("c.py")
        );
        assert_eq!(
            absolute_program_path(Path::new("/x/y/../z.py")),
            Path::new("/x/z.py")
        );
    }
}
