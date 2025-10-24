//! IO capture coordination for `RuntimeTracer`.

use crate::runtime::io_capture::{
    IoCapturePipeline, IoCaptureSettings, IoChunk, IoChunkFlags, IoStream, ScopedMuteIoCapture,
};
use crate::runtime::line_snapshots::{FrameId, LineSnapshotStore};
use pyo3::prelude::*;
use runtime_tracing::{
    EventLogKind, Line, NonStreamingTraceWriter, PathId, RecordEvent, TraceLowLevelEvent,
    TraceWriter,
};
use serde::Serialize;
use std::path::Path;
use std::sync::Arc;
use std::thread::ThreadId;

/// Coordinates installation, flushing, and teardown of the IO capture pipeline.
pub(crate) struct IoCoordinator {
    snapshots: Arc<LineSnapshotStore>,
    pipeline: Option<IoCapturePipeline>,
}

impl IoCoordinator {
    /// Create a coordinator with a fresh snapshot store and no active pipeline.
    pub(crate) fn new() -> Self {
        Self {
            snapshots: Arc::new(LineSnapshotStore::new()),
            pipeline: None,
        }
    }

    /// Expose the shared snapshot store for collaborators (tests, IO capture).
    pub(crate) fn snapshot_store(&self) -> Arc<LineSnapshotStore> {
        Arc::clone(&self.snapshots)
    }

    /// Install the IO capture pipeline using the provided settings.
    pub(crate) fn install(
        &mut self,
        py: Python<'_>,
        settings: IoCaptureSettings,
    ) -> PyResult<()> {
        self.pipeline = IoCapturePipeline::install(py, Arc::clone(&self.snapshots), settings)?;
        Ok(())
    }

    /// Flush buffered output for the active thread before emitting a step event.
    pub(crate) fn flush_before_step(
        &self,
        thread_id: ThreadId,
        writer: &mut NonStreamingTraceWriter,
    ) -> bool {
        let Some(pipeline) = self.pipeline.as_ref() else {
            return false;
        };

        pipeline.flush_before_step(thread_id);
        self.drain_chunks(pipeline, writer)
    }

    /// Flush every buffered chunk regardless of thread affinity.
    pub(crate) fn flush_all(&self, writer: &mut NonStreamingTraceWriter) -> bool {
        let Some(pipeline) = self.pipeline.as_ref() else {
            return false;
        };

        pipeline.flush_all();
        self.drain_chunks(pipeline, writer)
    }

    /// Drain remaining chunks and uninstall the capture pipeline.
    pub(crate) fn teardown(
        &mut self,
        py: Python<'_>,
        writer: &mut NonStreamingTraceWriter,
    ) -> bool {
        let Some(mut pipeline) = self.pipeline.take() else {
            return false;
        };

        pipeline.flush_all();
        let mut recorded = false;

        for chunk in pipeline.drain_chunks() {
            recorded |= self.record_chunk(writer, chunk);
        }

        pipeline.uninstall(py);

        for chunk in pipeline.drain_chunks() {
            recorded |= self.record_chunk(writer, chunk);
        }

        recorded
    }

    /// Clear the snapshot cache once tracing concludes.
    pub(crate) fn clear_snapshots(&self) {
        self.snapshots.clear();
    }

    /// Record the latest frame snapshot for the active thread.
    pub(crate) fn record_snapshot(
        &self,
        thread_id: ThreadId,
        path_id: PathId,
        line: Line,
        frame_id: FrameId,
    ) {
        self.snapshots.record(thread_id, path_id, line, frame_id);
    }

    fn drain_chunks(
        &self,
        pipeline: &IoCapturePipeline,
        writer: &mut NonStreamingTraceWriter,
    ) -> bool {
        let mut recorded = false;
        for chunk in pipeline.drain_chunks() {
            recorded |= self.record_chunk(writer, chunk);
        }
        recorded
    }

    fn record_chunk(&self, writer: &mut NonStreamingTraceWriter, mut chunk: IoChunk) -> bool {
        if chunk.path_id.is_none() {
            if let Some(path) = chunk.path.as_deref() {
                let path_id = TraceWriter::ensure_path_id(writer, Path::new(path));
                chunk.path_id = Some(path_id);
            }
        }

        let kind = match chunk.stream {
            IoStream::Stdout => EventLogKind::Write,
            IoStream::Stderr => EventLogKind::WriteOther,
            IoStream::Stdin => EventLogKind::Read,
        };

        let metadata = self.build_metadata(&chunk);
        let content = String::from_utf8_lossy(&chunk.payload).into_owned();

        TraceWriter::add_event(
            writer,
            TraceLowLevelEvent::Event(RecordEvent {
                kind,
                metadata,
                content,
            }),
        );

        true
    }

    fn build_metadata(&self, chunk: &IoChunk) -> String {
        #[derive(Serialize)]
        struct IoEventMetadata<'a> {
            stream: &'a str,
            thread: String,
            path_id: Option<usize>,
            line: Option<i64>,
            frame_id: Option<u64>,
            flags: Vec<&'a str>,
        }

        let snapshot = self.snapshots.snapshot_for_thread(chunk.thread_id);
        let path_id = chunk
            .path_id
            .map(|id| id.0)
            .or_else(|| snapshot.as_ref().map(|snap| snap.path_id().0));
        let line = chunk
            .line
            .map(|line| line.0)
            .or_else(|| snapshot.as_ref().map(|snap| snap.line().0));
        let frame_id = chunk
            .frame_id
            .or_else(|| snapshot.as_ref().map(|snap| snap.frame_id()));

        let metadata = IoEventMetadata {
            stream: match chunk.stream {
                IoStream::Stdout => "stdout",
                IoStream::Stderr => "stderr",
                IoStream::Stdin => "stdin",
            },
            thread: format!("{:?}", chunk.thread_id),
            path_id,
            line,
            frame_id: frame_id.map(|id| id.as_raw()),
            flags: flag_labels(chunk.flags),
        };

        match serde_json::to_string(&metadata) {
            Ok(json) => json,
            Err(err) => {
                let _mute = ScopedMuteIoCapture::new();
                log::error!("failed to serialise IO metadata: {err}");
                "{}".to_string()
            }
        }
    }
}

/// Translate chunk flags into telemetry labels.
fn flag_labels(flags: IoChunkFlags) -> Vec<&'static str> {
    let mut labels = Vec::new();
    if flags.contains(IoChunkFlags::NEWLINE_TERMINATED) {
        labels.push("newline");
    }
    if flags.contains(IoChunkFlags::EXPLICIT_FLUSH) {
        labels.push("flush");
    }
    if flags.contains(IoChunkFlags::STEP_BOUNDARY) {
        labels.push("step_boundary");
    }
    if flags.contains(IoChunkFlags::TIME_SPLIT) {
        labels.push("time_split");
    }
    if flags.contains(IoChunkFlags::INPUT_CHUNK) {
        labels.push("input");
    }
    if flags.contains(IoChunkFlags::FD_MIRROR) {
        labels.push("mirror");
    }
    labels
}
