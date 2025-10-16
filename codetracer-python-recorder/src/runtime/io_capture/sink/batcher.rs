use super::types::{IoChunk, IoChunkConsumer, IoChunkFlags};
use crate::runtime::io_capture::events::{IoOperation, IoStream, ProxyEvent};
use crate::runtime::line_snapshots::FrameId;
use runtime_tracing::{Line, PathId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

const MAX_BATCH_AGE: Duration = Duration::from_millis(5);

pub struct IoChunkBatcher {
    consumer: Arc<dyn IoChunkConsumer>,
    state: Mutex<IoSinkState>,
    time_source: Arc<dyn Fn() -> Instant + Send + Sync>,
}

impl IoChunkBatcher {
    pub fn new(consumer: Arc<dyn IoChunkConsumer>) -> Self {
        Self::with_time_source(consumer, Arc::new(Instant::now))
    }

    pub fn with_time_source(
        consumer: Arc<dyn IoChunkConsumer>,
        time_source: Arc<dyn Fn() -> Instant + Send + Sync>,
    ) -> Self {
        Self {
            consumer,
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

    pub fn handle_output(&self, mut event: ProxyEvent) {
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

    pub fn handle_input(&self, event: ProxyEvent) {
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
}

#[derive(Default)]
struct IoSinkState {
    threads: HashMap<ThreadId, ThreadBuffers>,
}

struct ThreadBuffers {
    stdout: StreamBuffer,
    stderr: StreamBuffer,
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

struct StreamBuffer {
    payload: Vec<u8>,
    last_timestamp: Option<Instant>,
    frame_id: Option<FrameId>,
    path_id: Option<PathId>,
    line: Option<Line>,
    path: Option<String>,
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
