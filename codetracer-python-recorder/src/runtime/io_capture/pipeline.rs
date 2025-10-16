//! Assembly and lifetime management for the IO capture pipeline.

use super::fd_mirror::{FdMirrorController, MirrorLedgers};
use super::install::IoStreamProxies;
use super::sink::{IoChunk, IoChunkConsumer, IoEventSink};
use crate::runtime::io_capture::events::ProxySink;
use crate::runtime::line_snapshots::LineSnapshotStore;
use pyo3::prelude::*;
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;

/// Configuration flags controlling which capture components install.
#[derive(Clone, Copy, Debug, Default)]
pub struct IoCaptureSettings {
    pub line_proxies: bool,
    pub fd_mirror: bool,
}

pub struct IoCapturePipeline {
    sink: Arc<IoEventSink>,
    buffer: Arc<IoChunkBuffer>,
    proxies: Option<IoStreamProxies>,
    fd_mirror: Option<FdMirrorController>,
}

impl IoCapturePipeline {
    /// Install the requested capture pipeline components.
    pub fn install(
        py: Python<'_>,
        snapshots: Arc<LineSnapshotStore>,
        settings: IoCaptureSettings,
    ) -> PyResult<Option<Self>> {
        if !settings.line_proxies {
            return Ok(None);
        }

        let buffer = Arc::new(IoChunkBuffer::new());
        let consumer: Arc<dyn IoChunkConsumer> = buffer.clone();

        let mirror_ledgers = if settings.fd_mirror {
            let ledgers = MirrorLedgers::new_enabled();
            if !ledgers.is_enabled() {
                log::warn!("fd_fallback requested but not supported on this platform");
                None
            } else {
                Some(ledgers)
            }
        } else {
            None
        };

        let sink = Arc::new(IoEventSink::new(consumer.clone(), Arc::clone(&snapshots)));
        let sink_for_proxies: Arc<dyn ProxySink> = sink.clone();
        let proxies = IoStreamProxies::install(py, sink_for_proxies, mirror_ledgers.clone())?;

        let fd_mirror = if let Some(ledgers) = mirror_ledgers {
            match FdMirrorController::new(ledgers.clone(), consumer.clone()) {
                Ok(controller) => Some(controller),
                Err(err) => {
                    log::warn!("failed to enable FD mirror fallback: {err}");
                    None
                }
            }
        } else {
            None
        };

        Ok(Some(Self {
            sink,
            buffer,
            proxies: Some(proxies),
            fd_mirror,
        }))
    }

    /// Flush buffered output for a specific thread before emitting a Step event.
    pub fn flush_before_step(&self, thread_id: ThreadId) {
        self.sink.flush_before_step(thread_id);
    }

    /// Drain all in-flight output regardless of thread affinity.
    pub fn flush_all(&self) {
        self.sink.flush_all();
    }

    /// Take ownership of the buffered IO chunks accumulated so far.
    pub fn drain_chunks(&self) -> Vec<IoChunk> {
        self.buffer.drain()
    }

    /// Restore the original IO streams and tear down the FD mirror, if present.
    pub fn uninstall(&mut self, py: Python<'_>) {
        if let Some(mut mirror) = self.fd_mirror.take() {
            mirror.shutdown();
        }
        if let Some(mut proxies) = self.proxies.take() {
            if let Err(err) = proxies.uninstall(py) {
                err.print(py);
            }
        }
    }
}

impl Drop for IoCapturePipeline {
    fn drop(&mut self) {
        Python::with_gil(|py| {
            self.uninstall(py);
        });
    }
}

struct IoChunkBuffer {
    queue: Mutex<Vec<IoChunk>>,
}

impl IoChunkBuffer {
    fn new() -> Self {
        Self {
            queue: Mutex::new(Vec::new()),
        }
    }

    fn drain(&self) -> Vec<IoChunk> {
        let mut guard = self.queue.lock().expect("io chunk buffer poisoned");
        std::mem::take(&mut *guard)
    }
}

impl IoChunkConsumer for IoChunkBuffer {
    fn consume(&self, chunk: IoChunk) {
        let mut guard = self.queue.lock().expect("io chunk buffer poisoned");
        guard.push(chunk);
    }
}
