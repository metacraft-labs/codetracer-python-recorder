//! Lifecycle orchestration for `RuntimeTracer`.

use crate::logging::set_active_trace_id;
use crate::policy::RecorderPolicy;
use crate::runtime::activation::ActivationController;
use crate::runtime::io_capture::ScopedMuteIoCapture;
use crate::runtime::output_paths::TraceOutputPaths;
use crate::runtime::tracer::filtering::FilterCoordinator;
use crate::runtime::tracer::runtime_tracer::ExitSummary;
use log::debug;
use recorder_errors::{enverr, usage, ErrorCode, RecorderResult};
use codetracer_trace_writer_nim::trace_writer::TraceWriter;
use serde_json::{self, json};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Coordinates writer setup, activation, and teardown flows.
#[derive(Debug)]
pub struct LifecycleController {
    activation: ActivationController,
    program_path: PathBuf,
    output_paths: Option<TraceOutputPaths>,
    events_recorded: bool,
    encountered_failure: bool,
    trace_id: String,
}

impl LifecycleController {
    pub fn new(program: &str, activation_path: Option<&Path>) -> Self {
        Self {
            activation: ActivationController::new(activation_path),
            program_path: PathBuf::from(program),
            output_paths: None,
            events_recorded: false,
            encountered_failure: false,
            trace_id: Uuid::new_v4().to_string(),
        }
    }

    #[cfg(test)]
    pub fn activation(&self) -> &ActivationController {
        &self.activation
    }

    pub fn activation_mut(&mut self) -> &mut ActivationController {
        &mut self.activation
    }

    pub fn begin(
        &mut self,
        writer: &mut dyn TraceWriter,
        outputs: &TraceOutputPaths,
        start_line: u32,
    ) -> RecorderResult<()> {
        let start_path = self.activation.start_path(&self.program_path);
        {
            let _mute = ScopedMuteIoCapture::new();
            log::debug!("{}", start_path.display());
        }
        outputs.configure_writer(writer, start_path, start_line)?;
        self.output_paths = Some(outputs.clone());
        self.events_recorded = false;
        self.encountered_failure = false;
        self.set_trace_id_active();
        Ok(())
    }

    pub fn mark_event(&mut self) {
        self.events_recorded = true;
    }

    pub fn mark_failure(&mut self) {
        self.encountered_failure = true;
    }

    pub fn encountered_failure(&self) -> bool {
        self.encountered_failure
    }

    pub fn require_trace_or_fail(&self, policy: &RecorderPolicy) -> RecorderResult<()> {
        if policy.require_trace && !self.events_recorded {
            return Err(usage!(
                ErrorCode::TraceMissing,
                "recorder policy requires a trace but no events were recorded"
            ));
        }
        Ok(())
    }

    pub fn cleanup_partial_outputs(&self) -> RecorderResult<()> {
        if let Some(outputs) = &self.output_paths {
            for path in [outputs.events(), outputs.metadata(), outputs.paths()] {
                if path.exists() {
                    fs::remove_file(path).map_err(|err| {
                        enverr!(ErrorCode::Io, "failed to remove partial trace file")
                            .with_context("path", path.display().to_string())
                            .with_context("io", err.to_string())
                    })?;
                }
            }
        }
        Ok(())
    }

    pub fn finalise(
        &mut self,
        writer: &mut dyn TraceWriter,
        filter: &FilterCoordinator,
        exit_summary: &ExitSummary,
    ) -> RecorderResult<()> {
        // TF-M7: thread the composed filter chain (builtin → auto-discovered
        // → env-var → CLI `--trace-filter:`) into the CTFS meta.dat block
        // BEFORE finish/close.  The Nim writer accumulates entries on the
        // handle and emits them in `close()`, so we must populate before
        // finish_*.
        Self::publish_filter_provenance(writer, filter)?;
        TraceWriter::finish_writing_trace_metadata(writer).map_err(|err| {
            enverr!(ErrorCode::Io, "failed to finalise trace metadata")
                .with_context("source", err.to_string())
        })?;
        TraceWriter::finish_writing_trace_paths(writer).map_err(|err| {
            enverr!(ErrorCode::Io, "failed to finalise trace paths")
                .with_context("source", err.to_string())
        })?;
        TraceWriter::finish_writing_trace_events(writer).map_err(|err| {
            enverr!(ErrorCode::Io, "failed to finalise trace events")
                .with_context("source", err.to_string())
        })?;
        debug!(
            "[Lifecycle] writing exit metadata: code={:?}, label={:?}",
            exit_summary.code, exit_summary.label
        );
        self.append_filter_metadata(filter)?;
        self.append_exit_metadata(exit_summary)?;
        TraceWriter::close(writer).map_err(|err| {
            enverr!(ErrorCode::Io, "failed to close trace writer")
                .with_context("source", err.to_string())
        })?;
        Ok(())
    }

    /// TF-M7 (spec § 7 / Trace-Filters.md § 7): forward each composed
    /// filter source's `(path, sha256)` pair to the CTFS writer so the
    /// resulting `meta.dat` records the filter chain.  Closes the
    /// regression flagged in `AUDIT-CTFS-2026-05.md`
    /// § "Known coverage regression — trace-filter chain assertions"
    /// where the pre-2026-05 JSON sidecar carried this provenance but
    /// the CTFS-only migration dropped it.
    ///
    /// `FilterSummaryEntry::sha256` is hex-encoded by the shared crate;
    /// we decode it back to the raw 32-byte digest the meta.dat wire
    /// format expects.  Invalid hex / wrong length would indicate a
    /// shared-crate bug, not a recorder one — propagate it loudly.
    fn publish_filter_provenance(
        writer: &mut dyn TraceWriter,
        filter: &FilterCoordinator,
    ) -> RecorderResult<()> {
        let Some(engine) = filter.engine() else {
            // Recorder ran without a filter coordinator (e.g. legacy
            // entry points that pass `None`).  Spec § 7 says do not set
            // the flag in that case — leave the writer untouched.
            return Ok(());
        };
        let summary = engine.summary();
        if summary.entries.is_empty() {
            // Spec § 7: a filter-aware recorder with an empty chain
            // SHOULD still emit a present-but-empty provenance block to
            // distinguish "did not record" from "recorded empty".
            writer.record_empty_filter_provenance().map_err(|err| {
                enverr!(ErrorCode::Io, "failed to record empty filter provenance")
                    .with_context("source", err.to_string())
            })?;
            return Ok(());
        }
        for entry in &summary.entries {
            let raw = decode_sha256_hex(&entry.sha256).map_err(|err| {
                enverr!(ErrorCode::Unknown, "invalid sha256 hex in filter summary")
                    .with_context("path", entry.path.display().to_string())
                    .with_context("sha256", entry.sha256.clone())
                    .with_context("source", err)
            })?;
            let path_str = entry.path.to_string_lossy();
            writer
                .add_filter_provenance(path_str.as_ref(), &raw)
                .map_err(|err| {
                    enverr!(ErrorCode::Io, "failed to record filter provenance")
                        .with_context("path", path_str.into_owned())
                        .with_context("source", err.to_string())
                })?;
        }
        Ok(())
    }

    pub fn output_paths(&self) -> Option<&TraceOutputPaths> {
        self.output_paths.as_ref()
    }

    pub fn reset_event_state(&mut self) {
        self.output_paths = None;
        self.events_recorded = false;
        self.encountered_failure = false;
    }

    fn append_exit_metadata(&self, exit_summary: &ExitSummary) -> RecorderResult<()> {
        let Some(outputs) = &self.output_paths else {
            return Ok(());
        };

        let path = outputs.metadata();
        // With the Nim CTFS writer, metadata lives inside the .ct container file
        // rather than as a separate JSON file. Skip gracefully if it doesn't exist.
        if !path.exists() {
            return Ok(());
        }
        let original = fs::read_to_string(path).map_err(|err| {
            enverr!(ErrorCode::Io, "failed to read trace metadata")
                .with_context("path", path.display().to_string())
                .with_context("source", err.to_string())
        })?;

        let mut metadata: serde_json::Value = serde_json::from_str(&original).map_err(|err| {
            enverr!(ErrorCode::Io, "failed to parse trace metadata JSON")
                .with_context("path", path.display().to_string())
                .with_context("source", err.to_string())
        })?;

        if let serde_json::Value::Object(ref mut obj) = metadata {
            let status = json!({
                "code": exit_summary.code,
                "label": exit_summary.label,
            });
            obj.insert("process_exit_status".to_string(), status);
            let serialised = serde_json::to_string(&metadata).map_err(|err| {
                enverr!(ErrorCode::Io, "failed to serialise trace metadata")
                    .with_context("path", path.display().to_string())
                    .with_context("source", err.to_string())
            })?;
            fs::write(path, serialised).map_err(|err| {
                enverr!(ErrorCode::Io, "failed to write trace metadata")
                    .with_context("path", path.display().to_string())
                    .with_context("source", err.to_string())
            })?;
            Ok(())
        } else {
            Err(
                enverr!(ErrorCode::Io, "trace metadata must be a JSON object")
                    .with_context("path", path.display().to_string()),
            )
        }
    }

    fn append_filter_metadata(&self, _filter: &FilterCoordinator) -> RecorderResult<()> {
        // TF-M7: the trace-filter chain is now written into the CTFS
        // `meta.dat` block by `publish_filter_provenance` (called
        // earlier in `finalise`).  The pre-CTFS-migration sidecar
        // approach this function used to take is no longer reachable —
        // `outputs.metadata()` does not exist as a separate file under
        // the CTFS-only contract.  Per-event filter *stats* (counts of
        // skipped scopes, redacted values, …) are still a useful
        // observability surface but the wire format spec § 7 explicitly
        // limits meta.dat to *provenance* (path + sha256) so we drop
        // the `stats` sidecar write rather than re-emit it under a key
        // that doesn't survive into the materializer.  If we ever
        // need recorder-side filter stats again, the right path is a
        // CTFS internal file (e.g. `filter_stats.json`) rather than
        // overloading meta.dat.
        Ok(())
    }

    fn set_trace_id_active(&self) {
        set_active_trace_id(Some(self.trace_id.clone()));
    }

    pub fn trace_id_scope(&self) -> TraceIdScope {
        self.set_trace_id_active();
        TraceIdScope
    }
}

/// TF-M7: decode a 64-character lowercase hex string (the shared
/// `codetracer_trace_filter` crate's canonical encoding for
/// `FilterSummaryEntry::sha256`) back into the raw 32-byte digest that
/// the CTFS `meta.dat` wire format expects.  Returns an error string
/// (not a structured `RecorderResult`) so the call site can layer its
/// own diagnostic context onto the failure.  Wrong length or
/// non-hexadecimal characters are caller-visible bugs in the shared
/// crate; we surface them rather than silently truncate / pad.
fn decode_sha256_hex(hex: &str) -> Result<[u8; 32], String> {
    if hex.len() != 64 {
        return Err(format!(
            "expected 64 hex characters, got {len}",
            len = hex.len()
        ));
    }
    let mut out = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let high = match chunk[0] {
            b'0'..=b'9' => chunk[0] - b'0',
            b'a'..=b'f' => chunk[0] - b'a' + 10,
            b'A'..=b'F' => chunk[0] - b'A' + 10,
            b => return Err(format!("invalid hex byte 0x{:02x} at index {}", b, i * 2)),
        };
        let low = match chunk[1] {
            b'0'..=b'9' => chunk[1] - b'0',
            b'a'..=b'f' => chunk[1] - b'a' + 10,
            b'A'..=b'F' => chunk[1] - b'A' + 10,
            b => {
                return Err(format!(
                    "invalid hex byte 0x{:02x} at index {}",
                    b,
                    i * 2 + 1
                ))
            }
        };
        out[i] = (high << 4) | low;
    }
    Ok(out)
}

/// Guard that clears the active trace id when dropped.
pub(crate) struct TraceIdScope;

impl Drop for TraceIdScope {
    fn drop(&mut self) {
        set_active_trace_id(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logging::{init_rust_logging_with_default, snapshot_run_and_trace};
    use crate::policy::RecorderPolicy;
    use crate::runtime::output_paths::TraceOutputPaths;
    use recorder_errors::ErrorCode;
    use codetracer_trace_writer_nim::non_streaming_trace_writer::NonStreamingTraceWriter;
    use codetracer_trace_writer_nim::TraceEventsFileFormat;

    fn writer() -> NonStreamingTraceWriter {
        NonStreamingTraceWriter::new("program.py", &[])
    }

    #[test]
    fn policy_requiring_trace_fails_without_events() {
        let controller = LifecycleController::new("program.py", None);
        let mut policy = RecorderPolicy::default();
        policy.require_trace = true;

        let err = controller.require_trace_or_fail(&policy).unwrap_err();
        assert_eq!(err.code, ErrorCode::TraceMissing);
    }

    #[test]
    fn policy_requiring_trace_passes_after_event() {
        let mut controller = LifecycleController::new("program.py", None);
        let mut policy = RecorderPolicy::default();
        policy.require_trace = true;
        controller.mark_event();

        assert!(controller.require_trace_or_fail(&policy).is_ok());
    }

    #[test]
    fn cleanup_removes_partial_outputs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let outputs = TraceOutputPaths::new(tmp.path(), TraceEventsFileFormat::Json);
        let mut controller = LifecycleController::new("program.py", None);
        let mut writer = writer();

        controller
            .begin(&mut writer, &outputs, 1)
            .expect("begin lifecycle");

        std::fs::write(outputs.events(), "events").expect("write events");
        std::fs::write(outputs.metadata(), "{}").expect("write metadata");
        std::fs::write(outputs.paths(), "[]").expect("write paths");

        controller
            .cleanup_partial_outputs()
            .expect("cleanup outputs");

        assert!(
            !outputs.events().exists(),
            "expected events file removed after cleanup"
        );
        assert!(
            !outputs.metadata().exists(),
            "expected metadata file removed after cleanup"
        );
        assert!(
            !outputs.paths().exists(),
            "expected paths file removed after cleanup"
        );
    }

    #[test]
    fn trace_id_scope_sets_and_clears_active_id() {
        init_rust_logging_with_default("codetracer_python_recorder=error");
        let controller = LifecycleController::new("program.py", None);

        {
            let _scope = controller.trace_id_scope();
            let (_, active) = snapshot_run_and_trace().expect("logger initialised");
            assert!(matches!(active.as_deref(), Some(id) if !id.is_empty()));
        }

        let (_, cleared) = snapshot_run_and_trace().expect("logger initialised");
        assert!(cleared.is_none(), "expected trace id cleared after scope");
    }
}
