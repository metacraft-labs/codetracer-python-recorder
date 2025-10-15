use crate::runtime::io_capture::events::IoStream;
use crate::runtime::io_capture::sink::{IoChunk, IoChunkConsumer, IoChunkFlags};
use log::warn;
use std::collections::VecDeque;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

#[derive(Debug)]
pub struct FdMirrorError {
    message: String,
}

impl FdMirrorError {
    pub fn new(msg: impl Into<String>) -> Self {
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
    seq: u64,
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
            seq,
            data: payload.to_vec(),
            offset: 0,
        };
        let mut guard = self.entries.lock().expect("ledger lock poisoned");
        guard.push_back(entry);
        LedgerTicket::new(Arc::clone(self), seq)
    }

    fn cancel_entry(&self, seq: u64) {
        let mut guard = self.entries.lock().expect("ledger lock poisoned");
        guard.retain(|entry| entry.seq != seq);
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

                // Partial match; advance by one byte.
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

    fn ledger_for(&self, stream: IoStream) -> Option<Arc<Ledger>> {
        match stream {
            IoStream::Stdout => Some(Arc::clone(&self.stdout)),
            IoStream::Stderr => Some(Arc::clone(&self.stderr)),
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
    target_fd: RawFd,
    preserved_fd: OwnedFd,
    ledger: Arc<Ledger>,
    join: Option<thread::JoinHandle<()>>,
    shutdown_trigger: Arc<ShutdownSignal>,
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

        let shutdown = Arc::new(ShutdownSignal::default());
        let thread_shutdown = shutdown.clone();
        let ledger_clone = ledger.clone();
        let consumer_clone = consumer.clone();

        let join = thread::Builder::new()
            .name(format!("codetracer-fd-mirror-{}", stream))
            .spawn(move || {
                mirror_loop(
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
            target_fd,
            preserved_fd,
            ledger,
            join: Some(join),
            shutdown_trigger: shutdown,
        })
    }

    fn shutdown(&mut self) {
        self.shutdown_trigger.request_shutdown();
        if unsafe { libc::dup2(self.preserved_fd.as_raw_fd(), self.target_fd) } < 0 {
            warn!("failed to restore fd {} after mirroring", self.target_fd);
        }
        if let Some(join) = self.join.take() {
            if let Err(err) = join.join() {
                warn!("mirror thread join failed: {err:?}");
            }
        }
        self.ledger.clear();
    }
}

impl Drop for StreamMirror {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub struct FdMirrorController {
    stdout: Option<StreamMirror>,
    stderr: Option<StreamMirror>,
}

impl FdMirrorController {
    pub fn new(
        set: Arc<MirrorLedgerSet>,
        consumer: Arc<dyn IoChunkConsumer>,
    ) -> Result<Self, FdMirrorError> {
        let stdout = set
            .ledger_for(IoStream::Stdout)
            .map(|ledger| StreamMirror::start(IoStream::Stdout, ledger, consumer.clone()))
            .transpose()?;
        let stderr = set
            .ledger_for(IoStream::Stderr)
            .map(|ledger| StreamMirror::start(IoStream::Stderr, ledger, consumer))
            .transpose()?;
        Ok(Self { stdout, stderr })
    }

    pub fn shutdown(&mut self) {
        if let Some(stream) = self.stdout.as_mut() {
            stream.shutdown();
        }
        if let Some(stream) = self.stderr.as_mut() {
            stream.shutdown();
        }
        self.stdout = None;
        self.stderr = None;
    }
}

impl Drop for FdMirrorController {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[derive(Default)]
struct ShutdownSignal {
    triggered: AtomicBool,
}

impl ShutdownSignal {
    fn request_shutdown(&self) {
        self.triggered.store(true, Ordering::Relaxed);
    }

    fn should_exit(&self) -> bool {
        self.triggered.load(Ordering::Relaxed)
    }
}

fn mirror_loop(
    stream: IoStream,
    ledger: Arc<Ledger>,
    consumer: Arc<dyn IoChunkConsumer>,
    read_fd: OwnedFd,
    forward_fd: OwnedFd,
    shutdown: Arc<ShutdownSignal>,
) {
    let mut buffer = vec![0u8; 8192];
    while !shutdown.should_exit() {
        let read = unsafe {
            libc::read(
                read_fd.as_raw_fd(),
                buffer.as_mut_ptr() as *mut libc::c_void,
                buffer.len(),
            )
        };
        if read < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            warn!("fd mirror read error on {stream}: {err}");
            break;
        }
        if read == 0 {
            break;
        }

        let payload = &buffer[..read as usize];
        let leftover = ledger.subtract_from_chunk(payload);
        if leftover.is_empty() {
            continue;
        }

        if let Err(err) = write_all(forward_fd.as_raw_fd(), &leftover) {
            warn!("fd mirror write back error on {stream}: {err}");
            break;
        }

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

fn write_all(fd: RawFd, mut data: &[u8]) -> io::Result<()> {
    while !data.is_empty() {
        let written = unsafe {
            libc::write(
                fd,
                data.as_ptr() as *const libc::c_void,
                data.len().min(isize::MAX as usize),
            )
        };
        if written < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if written == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "failed to write to preserved fd",
            ));
        }
        data = &data[written as usize..];
    }
    Ok(())
}
