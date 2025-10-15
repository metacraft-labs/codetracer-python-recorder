use crate::runtime::io_capture::events::IoStream;
use crate::runtime::line_snapshots::FrameId;
use bitflags::bitflags;
use runtime_tracing::{Line, PathId};
use std::thread::ThreadId;
use std::time::Instant;

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
        /// The chunk originated from the FD mirror fallback.
        const FD_MIRROR = 0b0010_0000;
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
