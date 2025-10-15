//! Optional file descriptor mirror that complements line-aware proxies.
//!
//! The mirror duplicates `stdout`/`stderr` descriptors and drains their output
//! through a background reader. Bytes already recorded by the proxies are
//! tracked in a ledger so the mirror only emits native writes that bypass the
//! proxy layer.

use super::events::IoStream;
use super::sink::{IoChunk, IoChunkConsumer};

use std::sync::Arc;

#[derive(Clone, Default)]
pub struct MirrorLedgers(Option<Arc<platform::MirrorLedgerSet>>);

impl MirrorLedgers {
    pub fn new_enabled() -> Self {
        #[cfg(unix)]
        {
            Self(Some(Arc::new(platform::MirrorLedgerSet::new())))
        }
        #[cfg(not(unix))]
        {
            Self(None)
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.0.is_some()
    }

    pub fn begin_proxy_write(&self, stream: IoStream, payload: &[u8]) -> Option<LedgerTicket> {
        self.0
            .as_ref()
            .and_then(|inner| inner.begin_proxy_write(stream, payload))
    }

    fn inner(&self) -> Option<Arc<platform::MirrorLedgerSet>> {
        self.0.clone()
    }
}

pub struct FdMirrorController {
    #[allow(dead_code)]
    inner: Option<platform::FdMirrorController>,
}

impl FdMirrorController {
    pub fn new(
        ledgers: MirrorLedgers,
        consumer: Arc<dyn IoChunkConsumer>,
    ) -> Result<Self, FdMirrorError> {
        let inner = if let Some(set) = ledgers.inner() {
            Some(platform::FdMirrorController::new(set, consumer)?)
        } else {
            None
        };
        Ok(Self { inner })
    }

    pub fn shutdown(&mut self) {
        if let Some(inner) = self.inner.as_mut() {
            inner.shutdown();
        }
        self.inner = None;
    }
}

impl Drop for FdMirrorController {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.as_mut() {
            inner.shutdown();
        }
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    use crate::runtime::io_capture::sink::IoChunkFlags;
    use log::warn;
    use std::collections::VecDeque;
    use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Instant;

    #[derive(Debug)]
    pub struct FdMirrorError {
        message: String,
    }

    impl FdMirrorError {
        fn new(msg: impl Into<String>) -> Self {
            Self {
                message: msg.into(),
            }
        }
    }

    impl std::fmt::Display for FdMirrorError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.message)
        }
    }

    impl std::error::Error for FdMirrorError {}

    #[derive(Debug)]
    struct LedgerEntry {
        _seq: u64,
        data: Vec<u8>,
        offset: usize,
    }

    impl LedgerEntry {
        fn remaining(&self) -> &[u8] {
            &self.data[self.offset..]
        }

        fn consume(&mut self, amount: usize) {
            self.offset = std::cmp::min(self.offset + amount, self.data.len());
        }

        fn is_spent(&self) -> bool {
            self.offset >= self.data.len()
        }
    }

    #[derive(Debug)]
    struct Ledger {
        next_seq: AtomicU64,
        entries: Mutex<VecDeque<LedgerEntry>>,
        matched_bytes: AtomicU64,
        mirrored_bytes: AtomicU64,
    }

    impl Ledger {
        fn new() -> Self {
            Self {
                next_seq: AtomicU64::new(0),
                entries: Mutex::new(VecDeque::new()),
                matched_bytes: AtomicU64::new(0),
                mirrored_bytes: AtomicU64::new(0),
            }
        }

        fn begin_entry(self: &Arc<Self>, payload: &[u8]) -> LedgerTicket {
            let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
            let entry = LedgerEntry {
                _seq: seq,
                data: payload.to_vec(),
                offset: 0,
            };
            let mut guard = self.entries.lock().expect("ledger lock poisoned");
            guard.push_back(entry);
            LedgerTicket::new(Arc::clone(self), seq)
        }

        fn cancel_entry(&self, seq: u64) {
            let mut guard = self.entries.lock().expect("ledger lock poisoned");
            guard.retain(|entry| entry._seq != seq);
        }

        fn subtract_from_chunk(&self, chunk: &[u8]) -> Vec<u8> {
            if chunk.is_empty() {
                return Vec::new();
            }

            let mut leftover = Vec::new();
            let mut guard = self.entries.lock().expect("ledger lock poisoned");
            let mut idx = 0usize;

            while idx < chunk.len() {
                if let Some(front) = guard.front_mut() {
                    let remaining = front.remaining();
                    if remaining.is_empty() {
                        guard.pop_front();
                        continue;
                    }

                    if chunk[idx] != remaining[0] {
                        leftover.push(chunk[idx]);
                        idx += 1;
                        continue;
                    }

                    let full_len = remaining.len();
                    let end_idx = idx + full_len;
                    if end_idx <= chunk.len() && &chunk[idx..end_idx] == remaining {
                        front.consume(full_len);
                        self.matched_bytes
                            .fetch_add(full_len as u64, Ordering::Relaxed);
                        idx = end_idx;
                        if front.is_spent() {
                            guard.pop_front();
                        }
                        continue;
                    }

                    let tail = &chunk[idx..];
                    if remaining.starts_with(tail) {
                        front.consume(tail.len());
                        self.matched_bytes
                            .fetch_add(tail.len() as u64, Ordering::Relaxed);
                        if front.is_spent() {
                            guard.pop_front();
                        }
                        break;
                    }

                    leftover.push(chunk[idx]);
                    idx += 1;
                } else {
                    leftover.extend_from_slice(&chunk[idx..]);
                    break;
                }
            }

            if !leftover.is_empty() {
                self.mirrored_bytes
                    .fetch_add(leftover.len() as u64, Ordering::Relaxed);
            }
            leftover
        }

        fn clear(&self) {
            let mut guard = self.entries.lock().expect("ledger lock poisoned");
            guard.clear();
        }
    }

    #[derive(Clone)]
    pub struct MirrorLedgerSet {
        stdout: Arc<Ledger>,
        stderr: Arc<Ledger>,
    }

    impl MirrorLedgerSet {
        pub fn new() -> Self {
            Self {
                stdout: Arc::new(Ledger::new()),
                stderr: Arc::new(Ledger::new()),
            }
        }

        pub fn begin_proxy_write(&self, stream: IoStream, payload: &[u8]) -> Option<LedgerTicket> {
            match stream {
                IoStream::Stdout => Some(self.stdout.begin_entry(payload)),
                IoStream::Stderr => Some(self.stderr.begin_entry(payload)),
                IoStream::Stdin => None,
            }
        }
    }

    pub struct LedgerTicket {
        ledger: Arc<Ledger>,
        seq: u64,
        committed: AtomicBool,
    }

    impl LedgerTicket {
        fn new(ledger: Arc<Ledger>, seq: u64) -> Self {
            Self {
                ledger,
                seq,
                committed: AtomicBool::new(false),
            }
        }

        pub fn commit(self) {
            self.committed.store(true, Ordering::Relaxed);
        }
    }

    impl Drop for LedgerTicket {
        fn drop(&mut self) {
            if !self.committed.load(Ordering::Relaxed) {
                self.ledger.cancel_entry(self.seq);
            }
        }
    }

    struct StreamMirror {
        stream: IoStream,
        target_fd: RawFd,
        preserved_fd: OwnedFd,
        ledger: Arc<Ledger>,
        join: Option<thread::JoinHandle<()>>,
        shutdown_trigger: Arc<platform_internals::ShutdownSignal>,
    }

    impl StreamMirror {
        fn start(
            stream: IoStream,
            ledger: Arc<Ledger>,
            consumer: Arc<dyn IoChunkConsumer>,
        ) -> Result<Self, FdMirrorError> {
            let target_fd = match stream {
                IoStream::Stdout => libc::STDOUT_FILENO,
                IoStream::Stderr => libc::STDERR_FILENO,
                IoStream::Stdin => {
                    return Err(FdMirrorError::new("stdin mirroring not supported"));
                }
            };

            let preserved = unsafe { libc::dup(target_fd) };
            if preserved < 0 {
                return Err(FdMirrorError::new("dup failed for target fd"));
            }
            let preserved_fd = unsafe { OwnedFd::from_raw_fd(preserved) };

            let mut pipe_fds = [0; 2];
            if unsafe { libc::pipe(pipe_fds.as_mut_ptr()) } != 0 {
                return Err(FdMirrorError::new("pipe setup failed"));
            }
            let read_fd = unsafe { OwnedFd::from_raw_fd(pipe_fds[0]) };
            let write_fd = pipe_fds[1];

            if unsafe { libc::dup2(write_fd, target_fd) } < 0 {
                unsafe {
                    libc::close(write_fd);
                }
                return Err(FdMirrorError::new("dup2 failed while installing mirror"));
            }

            unsafe {
                libc::close(write_fd);
            }

            let forward_fd = unsafe { libc::dup(preserved_fd.as_raw_fd()) };
            if forward_fd < 0 {
                return Err(FdMirrorError::new("dup failed for forward fd"));
            }
            let forward_owned = unsafe { OwnedFd::from_raw_fd(forward_fd) };

            let shutdown = Arc::new(platform_internals::ShutdownSignal::default());
            let thread_shutdown = shutdown.clone();
            let ledger_clone = ledger.clone();
            let consumer_clone = consumer.clone();

            let join = thread::Builder::new()
                .name(format!("codetracer-fd-mirror-{}", stream))
                .spawn(move || {
                    platform_internals::mirror_loop(
                        stream,
                        ledger_clone,
                        consumer_clone,
                        read_fd,
                        forward_owned,
                        thread_shutdown,
                    );
                })
                .map_err(|err| FdMirrorError::new(format!("spawn failed: {err}")))?;

            Ok(Self {
                stream,
                target_fd,
                preserved_fd,
                ledger,
                join: Some(join),
                shutdown_trigger: shutdown,
            })
        }

        fn shutdown(&mut self) {
            if unsafe { libc::dup2(self.preserved_fd.as_raw_fd(), self.target_fd) } < 0 {
                warn!(
                    "FdMirror failed to restore descriptor {} for {:?}",
                    self.target_fd, self.stream
                );
            }
            self.shutdown_trigger.request_shutdown();
            if let Some(handle) = self.join.take() {
                let _ = handle.join();
            }
            self.ledger.clear();
        }
    }

    pub struct FdMirrorController {
        mirrors: Vec<StreamMirror>,
    }

    impl FdMirrorController {
        pub fn new(
            ledgers: Arc<MirrorLedgerSet>,
            consumer: Arc<dyn IoChunkConsumer>,
        ) -> Result<Self, FdMirrorError> {
            let mut mirrors = Vec::new();
            for stream in [IoStream::Stdout, IoStream::Stderr] {
                let ledger = match stream {
                    IoStream::Stdout => ledgers.stdout.clone(),
                    IoStream::Stderr => ledgers.stderr.clone(),
                    _ => unreachable!(),
                };
                match StreamMirror::start(stream, ledger, consumer.clone()) {
                    Ok(mirror) => mirrors.push(mirror),
                    Err(err) => {
                        for mirror in mirrors.iter_mut() {
                            mirror.shutdown();
                        }
                        return Err(err);
                    }
                }
            }
            Ok(Self { mirrors })
        }

        pub fn shutdown(&mut self) {
            for mirror in self.mirrors.iter_mut() {
                mirror.shutdown();
            }
            self.mirrors.clear();
        }
    }

    impl Drop for FdMirrorController {
        fn drop(&mut self) {
            self.shutdown();
        }
    }

    mod platform_internals {
        use super::*;
        use std::io::{Read, Write};
        use std::sync::atomic::Ordering;

        #[derive(Default)]
        pub struct ShutdownSignal {
            flag: std::sync::atomic::AtomicBool,
        }

        impl ShutdownSignal {
            pub fn request_shutdown(&self) {
                self.flag.store(true, Ordering::SeqCst);
            }

            pub fn should_stop(&self) -> bool {
                self.flag.load(Ordering::SeqCst)
            }
        }

        pub fn mirror_loop(
            stream: IoStream,
            ledger: Arc<Ledger>,
            consumer: Arc<dyn IoChunkConsumer>,
            read_fd: OwnedFd,
            forward_fd: OwnedFd,
            shutdown: Arc<ShutdownSignal>,
        ) {
            let mut reader = unsafe { std::fs::File::from_raw_fd(read_fd.into_raw_fd()) };
            let mut writer = unsafe { std::fs::File::from_raw_fd(forward_fd.into_raw_fd()) };
            let mut buffer = [0u8; 4096];

            while !shutdown.should_stop() {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        if writer.write_all(&buffer[..n]).is_err() {
                            break;
                        }
                        let chunk = &buffer[..n];
                        let leftover = ledger.subtract_from_chunk(chunk);
                        if !leftover.is_empty() {
                            let chunk = IoChunk {
                                stream,
                                payload: leftover,
                                thread_id: thread::current().id(),
                                timestamp: Instant::now(),
                                frame_id: None,
                                path_id: None,
                                line: None,
                                path: None,
                                flags: IoChunkFlags::FD_MIRROR,
                            };
                            consumer.consume(chunk);
                        }
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        }
    }
}

#[cfg(not(unix))]
mod platform {
    use super::*;

    #[derive(Debug)]
    pub struct FdMirrorError {
        #[allow(dead_code)]
        pub message: String,
    }

    impl FdMirrorError {
        pub fn new(message: impl Into<String>) -> Self {
            Self {
                message: message.into(),
            }
        }
    }

    #[derive(Default)]
    pub struct MirrorLedgerSet;

    pub struct FdMirrorController;

    pub struct LedgerTicket;

    impl MirrorLedgerSet {
        pub fn new() -> Self {
            Self
        }

        pub fn begin_proxy_write(&self, _: IoStream, _: &[u8]) -> Option<LedgerTicket> {
            None
        }
    }

    impl FdMirrorController {
        pub fn new(
            _: Arc<MirrorLedgerSet>,
            _: Arc<dyn IoChunkConsumer>,
        ) -> Result<Self, FdMirrorError> {
            Ok(Self)
        }

        pub fn shutdown(&mut self) {}
    }

    impl LedgerTicket {
        pub fn commit(self) {}
    }
}

pub type FdMirrorError = platform::FdMirrorError;
pub type LedgerTicket = platform::LedgerTicket;
