use crate::runtime::line_snapshots::FrameId;
use pyo3::Python;
use std::fmt;
use std::thread::ThreadId;
use std::time::Instant;

/// Distinguishes the proxied streams.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoStream {
    Stdout,
    Stderr,
    Stdin,
}

impl fmt::Display for IoStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IoStream::Stdout => write!(f, "stdout"),
            IoStream::Stderr => write!(f, "stderr"),
            IoStream::Stdin => write!(f, "stdin"),
        }
    }
}

/// Operations surfaced by the proxies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoOperation {
    Write,
    Writelines,
    Flush,
    Read,
    ReadLine,
    ReadLines,
    ReadInto,
}

/// Raw proxy payload collected during Stage 1.
#[derive(Clone, Debug)]
pub struct ProxyEvent {
    pub stream: IoStream,
    pub operation: IoOperation,
    pub payload: Vec<u8>,
    pub thread_id: ThreadId,
    pub timestamp: Instant,
    pub frame_id: Option<FrameId>,
}

/// Sink for proxy events. Later stages swap in a real writer-backed implementation.
pub trait ProxySink: Send + Sync + 'static {
    fn record(&self, py: Python<'_>, event: ProxyEvent);
}

/// No-op sink for scenarios where IO capture is disabled but proxies must install.
pub struct NullSink;

impl ProxySink for NullSink {
    fn record(&self, _py: Python<'_>, _event: ProxyEvent) {}
}
