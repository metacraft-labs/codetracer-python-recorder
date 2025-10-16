mod batcher;
mod enricher;
mod types;

pub use batcher::IoChunkBatcher;
pub use enricher::EventEnricher;
pub use types::{IoChunk, IoChunkConsumer, IoChunkFlags};

use crate::runtime::io_capture::events::{IoStream, ProxyEvent, ProxySink};
use crate::runtime::line_snapshots::LineSnapshotStore;
use pyo3::Python;
use std::sync::Arc;
use std::thread::ThreadId;

/// Batching sink that groups proxy events into line-aware IO chunks.
pub struct IoEventSink {
    enricher: EventEnricher,
    batcher: IoChunkBatcher,
}

impl IoEventSink {
    pub fn new(consumer: Arc<dyn IoChunkConsumer>, snapshots: Arc<LineSnapshotStore>) -> Self {
        let enricher = EventEnricher::new(Arc::clone(&snapshots));
        let batcher = IoChunkBatcher::new(consumer);
        Self { enricher, batcher }
    }

    #[cfg(test)]
    pub(crate) fn with_time_source(
        consumer: Arc<dyn IoChunkConsumer>,
        snapshots: Arc<LineSnapshotStore>,
        time_source: Arc<dyn Fn() -> std::time::Instant + Send + Sync>,
    ) -> Self {
        let enricher = EventEnricher::new(Arc::clone(&snapshots));
        let batcher = IoChunkBatcher::with_time_source(consumer, time_source);
        Self { enricher, batcher }
    }

    fn handle_enriched_event(&self, event: ProxyEvent) {
        match event.stream {
            IoStream::Stdout | IoStream::Stderr => self.batcher.handle_output(event),
            IoStream::Stdin => self.batcher.handle_input(event),
        }
    }

    pub fn flush_before_step(&self, thread_id: ThreadId) {
        self.batcher.flush_before_step(thread_id);
    }

    pub fn flush_all(&self) {
        self.batcher.flush_all();
    }
}

impl ProxySink for IoEventSink {
    fn record(&self, py: Python<'_>, event: ProxyEvent) {
        if let Some(enriched) = self.enricher.enrich(py, event) {
            self.handle_enriched_event(enriched);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::line_snapshots::LineSnapshotStore;
    use runtime_tracing::{Line, PathId};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use crate::runtime::io_capture::events::IoOperation;

    #[derive(Default)]
    struct ChunkRecorder {
        chunks: Mutex<Vec<IoChunk>>,
    }

    impl ChunkRecorder {
        fn chunks(&self) -> Vec<IoChunk> {
            self.chunks.lock().expect("lock poisoned").clone()
        }
    }

    impl IoChunkConsumer for ChunkRecorder {
        fn consume(&self, chunk: IoChunk) {
            self.chunks.lock().expect("lock poisoned").push(chunk);
        }
    }

    fn make_write_event(
        thread_id: ThreadId,
        stream: IoStream,
        payload: &[u8],
        timestamp: Instant,
        path_id: PathId,
        line: Line,
    ) -> ProxyEvent {
        ProxyEvent {
            stream,
            operation: IoOperation::Write,
            payload: payload.to_vec(),
            thread_id,
            timestamp,
            frame_id: Some(crate::runtime::line_snapshots::FrameId::from_raw(42)),
            path_id: Some(path_id),
            line: Some(line),
            path: Some(format!("/tmp/test{}_{}.py", path_id.0, line.0)),
        }
    }

    #[test]
    fn sink_batches_until_newline_flushes() {
        Python::with_gil(|py| {
            let collector: Arc<ChunkRecorder> = Arc::new(ChunkRecorder::default());
            let snapshots = Arc::new(LineSnapshotStore::new());
            let sink = IoEventSink::new(collector.clone(), snapshots);
            let thread_id = thread::current().id();
            let base = Instant::now();

            sink.record(
                py,
                make_write_event(
                    thread_id,
                    IoStream::Stdout,
                    b"hello",
                    base,
                    PathId(1),
                    Line(10),
                ),
            );
            assert!(collector.chunks().is_empty());

            sink.record(
                py,
                make_write_event(
                    thread_id,
                    IoStream::Stdout,
                    b" world\ntrailing",
                    base + std::time::Duration::from_millis(1),
                    PathId(1),
                    Line(10),
                ),
            );

            let chunks = collector.chunks();
            assert_eq!(chunks.len(), 1);
            assert_eq!(chunks[0].payload, b"hello world\n");
            assert!(chunks[0].flags.contains(IoChunkFlags::NEWLINE_TERMINATED));
            assert_eq!(
                chunks[0].frame_id,
                Some(crate::runtime::line_snapshots::FrameId::from_raw(42))
            );
            assert_eq!(chunks[0].path_id, Some(PathId(1)));
            assert_eq!(chunks[0].line, Some(Line(10)));
            assert_eq!(chunks[0].path.as_deref(), Some("/tmp/test1_10.py"));

            sink.flush_before_step(thread_id);
            let chunks = collector.chunks();
            assert_eq!(chunks.len(), 2);
            assert_eq!(chunks[1].payload, b"trailing");
            assert!(chunks[1].flags.contains(IoChunkFlags::STEP_BOUNDARY));
            assert_eq!(chunks[1].path_id, Some(PathId(1)));
            assert_eq!(chunks[1].line, Some(Line(10)));
            assert_eq!(chunks[1].path.as_deref(), Some("/tmp/test1_10.py"));
        });
    }

    #[test]
    fn sink_flushes_on_time_gap() {
        Python::with_gil(|py| {
            let collector: Arc<ChunkRecorder> = Arc::new(ChunkRecorder::default());
            let snapshots = Arc::new(LineSnapshotStore::new());
            let sink = IoEventSink::new(collector.clone(), snapshots);
            let thread_id = thread::current().id();
            let base = Instant::now();

            sink.record(
                py,
                make_write_event(thread_id, IoStream::Stdout, b"a", base, PathId(2), Line(20)),
            );
            sink.record(
                py,
                make_write_event(
                    thread_id,
                    IoStream::Stdout,
                    b"b",
                    base + std::time::Duration::from_millis(10),
                    PathId(2),
                    Line(20),
                ),
            );

            let chunks = collector.chunks();
            assert_eq!(chunks.len(), 1);
            assert_eq!(chunks[0].payload, b"a");
            assert!(chunks[0].flags.contains(IoChunkFlags::TIME_SPLIT));
            assert_eq!(chunks[0].path_id, Some(PathId(2)));
            assert_eq!(chunks[0].line, Some(Line(20)));
            assert_eq!(chunks[0].path.as_deref(), Some("/tmp/test2_20.py"));

            sink.flush_before_step(thread_id);
            let chunks = collector.chunks();
            assert_eq!(chunks.len(), 2);
            assert_eq!(chunks[1].payload, b"b");
            assert_eq!(chunks[1].path_id, Some(PathId(2)));
            assert_eq!(chunks[1].line, Some(Line(20)));
            assert_eq!(chunks[1].path.as_deref(), Some("/tmp/test2_20.py"));
        });
    }

    #[test]
    fn sink_flushes_on_explicit_flush() {
        Python::with_gil(|py| {
            let collector: Arc<ChunkRecorder> = Arc::new(ChunkRecorder::default());
            let snapshots = Arc::new(LineSnapshotStore::new());
            let sink = IoEventSink::new(collector.clone(), snapshots);
            let thread_id = thread::current().id();
            let base = Instant::now();

            sink.record(
                py,
                ProxyEvent {
                    stream: IoStream::Stderr,
                    operation: IoOperation::Write,
                    payload: b"log".to_vec(),
                    thread_id,
                    timestamp: base,
                    frame_id: Some(crate::runtime::line_snapshots::FrameId::from_raw(99)),
                    path_id: Some(PathId(3)),
                    line: Some(Line(30)),
                    path: Some("/tmp/test3_30.py".to_string()),
                },
            );
            sink.record(
                py,
                ProxyEvent {
                    stream: IoStream::Stderr,
                    operation: IoOperation::Flush,
                    payload: Vec::new(),
                    thread_id,
                    timestamp: base + std::time::Duration::from_millis(1),
                    frame_id: Some(crate::runtime::line_snapshots::FrameId::from_raw(99)),
                    path_id: Some(PathId(3)),
                    line: Some(Line(30)),
                    path: Some("/tmp/test3_30.py".to_string()),
                },
            );

            let chunks = collector.chunks();
            assert_eq!(chunks.len(), 1);
            assert_eq!(chunks[0].payload, b"log");
            assert!(chunks[0].flags.contains(IoChunkFlags::EXPLICIT_FLUSH));
        });
    }

    #[test]
    fn flush_before_step_uses_custom_time_source() {
        Python::with_gil(|py| {
            let collector: Arc<ChunkRecorder> = Arc::new(ChunkRecorder::default());
            let snapshots = Arc::new(LineSnapshotStore::new());
            let fixed = Instant::now() + Duration::from_secs(5);
            let time_source = Arc::new(move || fixed);
            let sink = IoEventSink::with_time_source(collector.clone(), snapshots, time_source);
            let thread_id = thread::current().id();
            let base = Instant::now();

            sink.record(
                py,
                make_write_event(
                    thread_id,
                    IoStream::Stdout,
                    b"pending",
                    base,
                    PathId(7),
                    Line(70),
                ),
            );

            sink.flush_before_step(thread_id);

            let chunks = collector.chunks();
            assert_eq!(chunks.len(), 1);
            assert_eq!(chunks[0].timestamp, fixed);
        });
    }
}
