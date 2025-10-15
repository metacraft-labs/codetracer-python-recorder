use crate::runtime::io_capture::events::{IoOperation, IoStream, ProxyEvent, ProxySink};
use crate::runtime::io_capture::mute::is_io_capture_muted;
use crate::runtime::line_snapshots::{FrameId, LineSnapshotStore};
use bitflags::bitflags;
use pyo3::types::PyAnyMethods;
use pyo3::Python;
use runtime_tracing::{Line, PathId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

bitflags! {
    /// Additional metadata describing why a chunk flushed.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct IoChunkFlags: u8 {
        /// The buffer ended because a newline character was observed.
        const NEWLINE_TERMINATED = 0b0000_0001;
        /// The user triggered `flush()` on the underlying TextIOBase.
        const EXPLICIT_FLUSH = 0b0000_0010;
        /// The recorder forced a flush immediately before emitting a Step event.
        const STEP_BOUNDARY = 0b0000_0100;
        /// The buffer aged past the batching deadline.
        const TIME_SPLIT = 0b0000_1000;
        /// The chunk represents stdin data flowing into the program.
        const INPUT_CHUNK = 0b0001_0000;
    }
}

/// Normalised chunk emitted by the batching sink.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct IoChunk {
    pub stream: IoStream,
    pub payload: Vec<u8>,
    pub thread_id: ThreadId,
    pub timestamp: Instant,
    pub frame_id: Option<FrameId>,
    pub path_id: Option<PathId>,
    pub line: Option<Line>,
    pub path: Option<String>,
    pub flags: IoChunkFlags,
}

/// Consumer invoked when the sink emits a chunk.
pub trait IoChunkConsumer: Send + Sync + 'static {
    fn consume(&self, chunk: IoChunk);
}

const MAX_BATCH_AGE: Duration = Duration::from_millis(5);

#[allow(dead_code)]
/// Batching sink that groups proxy events into line-aware IO chunks.
pub struct IoEventSink {
    consumer: Arc<dyn IoChunkConsumer>,
    snapshots: Arc<LineSnapshotStore>,
    state: Mutex<IoSinkState>,
    time_source: Arc<dyn Fn() -> Instant + Send + Sync>,
}

struct IoSinkState {
    threads: HashMap<ThreadId, ThreadBuffers>,
}

struct ThreadBuffers {
    stdout: StreamBuffer,
    stderr: StreamBuffer,
}

struct StreamBuffer {
    payload: Vec<u8>,
    last_timestamp: Option<Instant>,
    frame_id: Option<FrameId>,
    path_id: Option<PathId>,
    line: Option<Line>,
    path: Option<String>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl IoEventSink {
    pub fn new(consumer: Arc<dyn IoChunkConsumer>, snapshots: Arc<LineSnapshotStore>) -> Self {
        Self::with_time_source(consumer, snapshots, Arc::new(Instant::now))
    }

    fn with_time_source(
        consumer: Arc<dyn IoChunkConsumer>,
        snapshots: Arc<LineSnapshotStore>,
        time_source: Arc<dyn Fn() -> Instant + Send + Sync>,
    ) -> Self {
        Self {
            consumer,
            snapshots,
            state: Mutex::new(IoSinkState::default()),
            time_source,
        }
    }

    fn now(&self) -> Instant {
        (self.time_source)()
    }

    pub fn flush_before_step(&self, thread_id: ThreadId) {
        let timestamp = self.now();
        let mut state = self.state.lock().expect("lock poisoned");
        if let Some(buffers) = state.threads.get_mut(&thread_id) {
            buffers.flush_all(
                thread_id,
                timestamp,
                IoChunkFlags::STEP_BOUNDARY,
                &*self.consumer,
            );
        }
    }

    pub fn flush_all(&self) {
        let timestamp = self.now();
        let mut state = self.state.lock().expect("lock poisoned");
        for (thread_id, buffers) in state.threads.iter_mut() {
            buffers.flush_all(
                *thread_id,
                timestamp,
                IoChunkFlags::STEP_BOUNDARY,
                &*self.consumer,
            );
        }
    }

    fn handle_output(&self, mut event: ProxyEvent) {
        let mut state = self.state.lock().expect("lock poisoned");
        let buffers = state
            .threads
            .entry(event.thread_id)
            .or_insert_with(ThreadBuffers::new);
        let buffer = buffers.buffer_mut(event.stream);

        if buffer.is_stale(event.timestamp) {
            let flush_timestamp = buffer.last_timestamp.unwrap_or(event.timestamp);
            buffer.emit(
                event.thread_id,
                event.stream,
                flush_timestamp,
                IoChunkFlags::TIME_SPLIT,
                &*self.consumer,
            );
        }

        match event.operation {
            IoOperation::Write | IoOperation::Writelines => {
                if event.payload.is_empty() {
                    return;
                }
                buffer.append(
                    &event.payload,
                    event.frame_id,
                    event.path_id,
                    event.line,
                    event.path.take(),
                    event.timestamp,
                );
                buffer.flush_complete_lines(
                    event.thread_id,
                    event.stream,
                    event.timestamp,
                    &*self.consumer,
                );
            }
            IoOperation::Flush => {
                buffer.emit(
                    event.thread_id,
                    event.stream,
                    event.timestamp,
                    IoChunkFlags::EXPLICIT_FLUSH,
                    &*self.consumer,
                );
            }
            _ => {}
        }
    }

    fn handle_input(&self, event: ProxyEvent) {
        if event.payload.is_empty() {
            return;
        }
        let chunk = IoChunk {
            stream: IoStream::Stdin,
            payload: event.payload,
            thread_id: event.thread_id,
            timestamp: event.timestamp,
            frame_id: event.frame_id,
            path_id: event.path_id,
            line: event.line,
            path: event.path,
            flags: IoChunkFlags::INPUT_CHUNK,
        };
        self.consumer.consume(chunk);
    }

    fn populate_from_stack(&self, py: Python<'_>, event: &mut ProxyEvent) {
        if event.line.is_some() && (event.path_id.is_some() || event.path.is_some()) {
            return;
        }

        let frame_result = (|| {
            let sys = py.import("sys")?;
            sys.getattr("_getframe")
        })();

        let getframe = match frame_result {
            Ok(obj) => obj,
            Err(_) => return,
        };

        for depth in [2_i32, 1, 0] {
            let frame_obj = match getframe.call1((depth,)) {
                Ok(frame) => frame,
                Err(_) => continue,
            };

            let frame = frame_obj;

            if event.line.is_none() {
                if let Ok(lineno) = frame
                    .getattr("f_lineno")
                    .and_then(|obj| obj.extract::<i32>())
                {
                    event.line = Some(Line(lineno as i64));
                }
            }

            if event.path.is_none() {
                if let Ok(code) = frame.getattr("f_code") {
                    if let Ok(filename) = code
                        .getattr("co_filename")
                        .and_then(|obj| obj.extract::<String>())
                    {
                        event.path = Some(filename);
                    }
                }
            }

            if event.frame_id.is_none() {
                let raw = frame.as_ptr() as usize as u64;
                event.frame_id = Some(FrameId::from_raw(raw));
            }

            if event.line.is_some() && (event.path_id.is_some() || event.path.is_some()) {
                break;
            }
        }
    }
}

impl ProxySink for IoEventSink {
    fn record(&self, py: Python<'_>, event: ProxyEvent) {
        if is_io_capture_muted() {
            return;
        }

        let mut event = event;
        if event.frame_id.is_none() || event.path_id.is_none() || event.line.is_none() {
            if let Some(snapshot) = self.snapshots.snapshot_for_thread(event.thread_id) {
                if event.frame_id.is_none() {
                    event.frame_id = Some(snapshot.frame_id());
                }
                if event.path_id.is_none() {
                    event.path_id = Some(snapshot.path_id());
                }
                if event.line.is_none() {
                    event.line = Some(snapshot.line());
                }
            }
        }

        if event.line.is_none() || (event.path_id.is_none() && event.path.is_none()) {
            self.populate_from_stack(py, &mut event);
        }

        match event.stream {
            IoStream::Stdout | IoStream::Stderr => self.handle_output(event),
            IoStream::Stdin => self.handle_input(event),
        }
    }
}

impl Default for IoSinkState {
    fn default() -> Self {
        Self {
            threads: HashMap::new(),
        }
    }
}

impl ThreadBuffers {
    fn new() -> Self {
        Self {
            stdout: StreamBuffer::new(),
            stderr: StreamBuffer::new(),
        }
    }

    fn buffer_mut(&mut self, stream: IoStream) -> &mut StreamBuffer {
        match stream {
            IoStream::Stdout => &mut self.stdout,
            IoStream::Stderr => &mut self.stderr,
            IoStream::Stdin => panic!("stdin does not use output buffers"),
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn flush_all(
        &mut self,
        thread_id: ThreadId,
        timestamp: Instant,
        flags: IoChunkFlags,
        consumer: &dyn IoChunkConsumer,
    ) {
        for stream in [IoStream::Stdout, IoStream::Stderr] {
            let buffer = self.buffer_mut(stream);
            buffer.emit(thread_id, stream, timestamp, flags, consumer);
        }
    }
}

impl StreamBuffer {
    fn new() -> Self {
        Self {
            payload: Vec::new(),
            last_timestamp: None,
            frame_id: None,
            path_id: None,
            line: None,
            path: None,
        }
    }

    fn append(
        &mut self,
        payload: &[u8],
        frame_id: Option<FrameId>,
        path_id: Option<PathId>,
        line: Option<Line>,
        path: Option<String>,
        timestamp: Instant,
    ) {
        if let Some(id) = frame_id {
            self.frame_id = Some(id);
        }
        if let Some(id) = path_id {
            self.path_id = Some(id);
        }
        if let Some(line) = line {
            self.line = Some(line);
        }
        if let Some(path) = path {
            self.path = Some(path);
        }
        self.payload.extend_from_slice(payload);
        self.last_timestamp = Some(timestamp);
    }

    fn take_all(&mut self) -> Option<Vec<u8>> {
        if self.payload.is_empty() {
            return None;
        }
        Some(std::mem::take(&mut self.payload))
    }

    fn emit(
        &mut self,
        thread_id: ThreadId,
        stream: IoStream,
        timestamp: Instant,
        flags: IoChunkFlags,
        consumer: &dyn IoChunkConsumer,
    ) {
        if let Some(payload) = self.take_all() {
            let chunk = IoChunk {
                stream,
                payload,
                thread_id,
                timestamp,
                frame_id: self.frame_id,
                path_id: self.path_id,
                line: self.line,
                path: self.path.take(),
                flags,
            };
            self.frame_id = None;
            self.path_id = None;
            self.line = None;
            self.path = None;
            self.last_timestamp = Some(timestamp);
            consumer.consume(chunk);
        }
    }

    fn flush_complete_lines(
        &mut self,
        thread_id: ThreadId,
        stream: IoStream,
        timestamp: Instant,
        consumer: &dyn IoChunkConsumer,
    ) {
        while let Some(index) = self.payload.iter().position(|byte| *byte == b'\n') {
            let prefix: Vec<u8> = self.payload.drain(..=index).collect();
            let chunk = IoChunk {
                stream,
                payload: prefix,
                thread_id,
                timestamp,
                frame_id: self.frame_id,
                path_id: self.path_id,
                line: self.line,
                path: self.path.clone(),
                flags: IoChunkFlags::NEWLINE_TERMINATED,
            };
            consumer.consume(chunk);
            if self.payload.is_empty() {
                self.frame_id = None;
                self.path_id = None;
                self.line = None;
                self.path = None;
            }
            self.last_timestamp = Some(timestamp);
        }
    }

    fn is_stale(&self, now: Instant) -> bool {
        if self.payload.is_empty() {
            return false;
        }
        match self.last_timestamp {
            Some(last) => now
                .checked_duration_since(last)
                .map(|elapsed| elapsed >= MAX_BATCH_AGE)
                .unwrap_or(false),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::line_snapshots::LineSnapshotStore;
    use std::sync::Mutex;
    use std::thread;

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
            frame_id: Some(FrameId::from_raw(42)),
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
                    base + Duration::from_millis(1),
                    PathId(1),
                    Line(10),
                ),
            );

            let chunks = collector.chunks();
            assert_eq!(chunks.len(), 1);
            assert_eq!(chunks[0].payload, b"hello world\n");
            assert!(chunks[0].flags.contains(IoChunkFlags::NEWLINE_TERMINATED));
            assert_eq!(chunks[0].frame_id, Some(FrameId::from_raw(42)));
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
                    base + Duration::from_millis(10),
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
                make_write_event(
                    thread_id,
                    IoStream::Stderr,
                    b"log",
                    base,
                    PathId(5),
                    Line(50),
                ),
            );

            sink.record(
                py,
                ProxyEvent {
                    stream: IoStream::Stderr,
                    operation: IoOperation::Flush,
                    payload: Vec::new(),
                    thread_id,
                    timestamp: base + Duration::from_millis(1),
                    frame_id: Some(FrameId::from_raw(7)),
                    path_id: Some(PathId(5)),
                    line: Some(Line(50)),
                    path: Some("/tmp/stderr.py".to_string()),
                },
            );

            let chunks = collector.chunks();
            assert_eq!(chunks.len(), 1);
            assert_eq!(chunks[0].payload, b"log");
            assert!(chunks[0].flags.contains(IoChunkFlags::EXPLICIT_FLUSH));
            assert_eq!(chunks[0].path_id, Some(PathId(5)));
            assert_eq!(chunks[0].line, Some(Line(50)));
            assert_eq!(chunks[0].path.as_deref(), Some("/tmp/test5_50.py"));
        });
    }

    #[test]
    fn sink_records_stdin_directly() {
        Python::with_gil(|py| {
            let collector: Arc<ChunkRecorder> = Arc::new(ChunkRecorder::default());
            let snapshots = Arc::new(LineSnapshotStore::new());
            let sink = IoEventSink::new(collector.clone(), snapshots);
            let thread_id = thread::current().id();
            let base = Instant::now();

            sink.record(
                py,
                ProxyEvent {
                    stream: IoStream::Stdin,
                    operation: IoOperation::ReadLine,
                    payload: b"input\n".to_vec(),
                    thread_id,
                    timestamp: base,
                    frame_id: Some(FrameId::from_raw(99)),
                    path_id: Some(PathId(3)),
                    line: Some(Line(30)),
                    path: Some("/tmp/stdin.py".to_string()),
                },
            );

            let chunks = collector.chunks();
            assert_eq!(chunks.len(), 1);
            assert_eq!(chunks[0].payload, b"input\n");
            assert!(chunks[0].flags.contains(IoChunkFlags::INPUT_CHUNK));
            assert_eq!(chunks[0].frame_id, Some(FrameId::from_raw(99)));
            assert_eq!(chunks[0].path_id, Some(PathId(3)));
            assert_eq!(chunks[0].line, Some(Line(30)));
            assert_eq!(chunks[0].path.as_deref(), Some("/tmp/stdin.py"));
        });
    }

    #[test]
    fn sink_populates_metadata_from_snapshots() {
        Python::with_gil(|py| {
            let collector: Arc<ChunkRecorder> = Arc::new(ChunkRecorder::default());
            let snapshots = Arc::new(LineSnapshotStore::new());
            let sink = IoEventSink::new(collector.clone(), Arc::clone(&snapshots));
            let thread_id = thread::current().id();
            let base = Instant::now();

            snapshots.record(thread_id, PathId(9), Line(90), FrameId::from_raw(900));

            sink.record(
                py,
                ProxyEvent {
                    stream: IoStream::Stdout,
                    operation: IoOperation::Write,
                    payload: b"auto\n".to_vec(),
                    thread_id,
                    timestamp: base,
                    frame_id: None,
                    path_id: None,
                    line: None,
                    path: None,
                },
            );

            let chunks = collector.chunks();
            assert_eq!(chunks.len(), 1);
            assert_eq!(chunks[0].payload, b"auto\n");
            assert_eq!(chunks[0].frame_id, Some(FrameId::from_raw(900)));
            assert_eq!(chunks[0].path_id, Some(PathId(9)));
            assert_eq!(chunks[0].line, Some(Line(90)));
        });
    }
}
