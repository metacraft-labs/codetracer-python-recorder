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
use runtime_tracing::{NonStreamingTraceWriter, TraceWriter};
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
        writer: &mut NonStreamingTraceWriter,
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
        writer: &mut NonStreamingTraceWriter,
        filter: &FilterCoordinator,
        exit_summary: &ExitSummary,
    ) -> RecorderResult<()> {
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
        debug!("[Lifecycle] writing exit metadata: code={:?}, label={:?}", exit_summary.code, exit_summary.label);
        self.append_filter_metadata(filter)?;
        self.append_exit_metadata(exit_summary)?;
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

    fn append_filter_metadata(&self, filter: &FilterCoordinator) -> RecorderResult<()> {
        let Some(outputs) = &self.output_paths else {
            return Ok(());
        };
        let Some(engine) = filter.engine() else {
            return Ok(());
        };

        let path = outputs.metadata();
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

        let filters = engine.summary();
        let filters_json: Vec<serde_json::Value> = filters
            .entries
            .iter()
            .map(|entry| {
                json!({
                    "path": entry.path.to_string_lossy(),
                    "sha256": entry.sha256,
                    "name": entry.name,
                    "version": entry.version,
                })
            })
            .collect();

        if let serde_json::Value::Object(ref mut obj) = metadata {
            obj.insert(
                "trace_filter".to_string(),
                json!({
                    "filters": filters_json,
                    "stats": filter.summary_json(),
                }),
            );
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

    fn set_trace_id_active(&self) {
        set_active_trace_id(Some(self.trace_id.clone()));
    }

    pub fn trace_id_scope(&self) -> TraceIdScope {
        self.set_trace_id_active();
        TraceIdScope
    }
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
    use runtime_tracing::{NonStreamingTraceWriter, TraceEventsFileFormat};

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
