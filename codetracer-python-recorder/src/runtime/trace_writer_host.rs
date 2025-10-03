//! Thread-safe wrapper around `NonStreamingTraceWriter`.

use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use log::warn;
use recorder_errors::{enverr, ErrorCode};
use runtime_tracing::{Line, NonStreamingTraceWriter, TraceEventsFileFormat, TraceWriter};

use crate::errors::Result;

use super::output_paths::TraceOutputPaths;

/// Shared facade for the runtime trace writer.
#[derive(Clone)]
pub struct TraceWriterHost {
    inner: Arc<Mutex<NonStreamingTraceWriter>>,
}

/// Guard exposing mutable access to the underlying writer.
pub struct TraceWriterGuard<'a> {
    guard: MutexGuard<'a, NonStreamingTraceWriter>,
}

impl TraceWriterHost {
    /// Build a new writer host configured for the current program.
    pub fn new(program: &str, args: &[String], format: TraceEventsFileFormat) -> Self {
        let mut writer = NonStreamingTraceWriter::new(program, args);
        writer.set_format(format);
        Self {
            inner: Arc::new(Mutex::new(writer)),
        }
    }

    /// Borrow the underlying writer. Recovers from poisoned mutexes while logging a warning.
    pub fn lock(&self) -> Result<TraceWriterGuard<'_>> {
        match self.inner.lock() {
            Ok(guard) => Ok(TraceWriterGuard { guard }),
            Err(poisoned) => {
                warn!("TraceWriterHost mutex poisoned; continuing with recovered state");
                Ok(TraceWriterGuard {
                    guard: poisoned.into_inner(),
                })
            }
        }
    }

    /// Configure output files and start trace recording.
    pub fn configure_outputs(
        &self,
        outputs: &TraceOutputPaths,
        start_path: &Path,
        start_line: u32,
    ) -> Result<()> {
        let mut writer = self.lock()?;
        outputs.configure_writer(&mut *writer, start_path, start_line)
    }

    /// Finalise metadata, path, and event files.
    pub fn finalize(&self) -> Result<()> {
        let mut writer = self.lock()?;
        TraceWriter::finish_writing_trace_metadata(&mut *writer).map_err(|err| {
            enverr!(ErrorCode::Io, "failed to finalise trace metadata")
                .with_context("source", err.to_string())
        })?;
        TraceWriter::finish_writing_trace_paths(&mut *writer).map_err(|err| {
            enverr!(ErrorCode::Io, "failed to finalise trace paths")
                .with_context("source", err.to_string())
        })?;
        TraceWriter::finish_writing_trace_events(&mut *writer).map_err(|err| {
            enverr!(ErrorCode::Io, "failed to finalise trace events")
                .with_context("source", err.to_string())
        })?;
        Ok(())
    }

    /// Finalise event data only (used by flush operations).
    pub fn finalize_events(&self) -> Result<()> {
        let mut writer = self.lock()?;
        TraceWriter::finish_writing_trace_events(&mut *writer).map_err(|err| {
            enverr!(ErrorCode::Io, "failed to finalise trace events")
                .with_context("source", err.to_string())
        })?;
        Ok(())
    }
}

impl<'a> Deref for TraceWriterGuard<'a> {
    type Target = NonStreamingTraceWriter;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl<'a> DerefMut for TraceWriterGuard<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

/// Convenience helper for recording individual steps with a borrowed guard.
pub fn register_step_with_guard(
    guard: &mut TraceWriterGuard<'_>,
    path: &Path,
    line: Line,
) -> runtime_tracing::PathId {
    let path_id = TraceWriter::ensure_path_id(&mut **guard, path);
    TraceWriter::register_step(&mut **guard, path, line);
    path_id
}
