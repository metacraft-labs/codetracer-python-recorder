use crate::runtime::io_capture::events::IoStream;
use crate::runtime::io_capture::sink::{IoChunk, IoChunkConsumer, IoChunkFlags};
use log::warn;
use std::collections::VecDeque;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

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
    const SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(250);
    const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(10);

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
        // Restore the target fd BEFORE signalling the reader. `dup2` closes
        // the target's reference to the pipe's write end, so once it returns
        // every byte written through the target fd is already in the pipe
        // buffer and no new byte can enter it through that fd. The reader
        // drains the pipe when it sees the signal (see `mirror_loop`), so the
        // drain is guaranteed to cover everything written before shutdown.
        // Signalling first would let the reader drain and exit while writes
        // through the target fd could still land in a pipe nobody reads.
        if unsafe { libc::dup2(self.preserved_fd.as_raw_fd(), self.target_fd) } < 0 {
            warn!("failed to restore fd {} after mirroring", self.target_fd);
        }
        self.shutdown_trigger.request_shutdown();
        if let Some(join) = self.join.take() {
            if wait_for_join(&join, Self::SHUTDOWN_TIMEOUT, Self::SHUTDOWN_POLL_INTERVAL) {
                if let Err(err) = join.join() {
                    warn!("mirror thread join failed: {err:?}");
                }
            } else {
                warn!(
                    "mirror thread on fd {} did not stop within {:?}; detaching background reader",
                    self.target_fd,
                    Self::SHUTDOWN_TIMEOUT
                );
                drop(join);
            }
        }
        self.ledger.clear();
    }
}

fn wait_for_join(
    handle: &thread::JoinHandle<()>,
    timeout: Duration,
    poll_interval: Duration,
) -> bool {
    if handle.is_finished() {
        return true;
    }
    if timeout.is_zero() {
        return false;
    }
    let mut remaining = timeout;
    loop {
        if handle.is_finished() {
            return true;
        }
        let sleep_for = std::cmp::min(poll_interval, remaining);
        thread::sleep(sleep_for);
        if handle.is_finished() {
            return true;
        }
        if remaining <= sleep_for {
            return false;
        }
        remaining -= sleep_for;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records every chunk the mirror delivers. A real consumer (the
    /// pipeline's `IoChunkBuffer`) would do the same; this one lets the test
    /// read the chunks back without building a pipeline.
    #[derive(Default)]
    struct CollectingConsumer {
        chunks: Mutex<Vec<IoChunk>>,
    }

    impl IoChunkConsumer for CollectingConsumer {
        fn consume(&self, chunk: IoChunk) {
            self.chunks.lock().expect("collector lock").push(chunk);
        }
    }

    /// A pipe whose read end goes to `mirror_loop`; the write end is returned
    /// so a test decides when (or whether) it closes. The loop forwards to
    /// /dev/null, which stands in for the real stream the target fd pointed at.
    fn pipe_with_payload(payload: &[u8]) -> (OwnedFd, OwnedFd, OwnedFd) {
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0, "pipe");
        let read_fd = unsafe { OwnedFd::from_raw_fd(fds[0]) };
        let write_fd = unsafe { OwnedFd::from_raw_fd(fds[1]) };
        write_all(write_fd.as_raw_fd(), payload).expect("fill pipe");
        let forward = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/null")
            .expect("open /dev/null");
        (read_fd, write_fd, OwnedFd::from(forward))
    }

    fn delivered(consumer: &CollectingConsumer) -> Vec<u8> {
        consumer
            .chunks
            .lock()
            .expect("collector lock")
            .iter()
            .flat_map(|chunk| chunk.payload.clone())
            .collect()
    }

    /// Shutdown requested before the reader ever ran: the case a loaded host
    /// produces when the reader thread is scheduled late. Bytes already in
    /// the pipe must still be delivered (the loop used to return at once).
    #[test]
    fn mirror_loop_delivers_data_written_before_shutdown_was_observed() {
        let (read_fd, write_fd, forward) = pipe_with_payload(b"written before shutdown\n");
        drop(write_fd); // as `StreamMirror::shutdown` does by restoring the fd
        let shutdown = Arc::new(ShutdownSignal::default());
        shutdown.request_shutdown();
        let consumer = Arc::new(CollectingConsumer::default());

        mirror_loop(
            IoStream::Stdout,
            Arc::new(Ledger::new()),
            consumer.clone(),
            read_fd,
            forward,
            shutdown,
        );

        assert_eq!(delivered(&consumer), b"written before shutdown\n");
    }

    /// A write end still open after shutdown (a child process inherited it)
    /// keeps end-of-file from arriving. The loop must deliver what is in the
    /// pipe and return rather than block on the next read.
    #[test]
    fn mirror_loop_drains_and_returns_while_an_inherited_writer_is_open() {
        let (read_fd, write_fd, forward) = pipe_with_payload(b"pending\n");
        let shutdown = Arc::new(ShutdownSignal::default());
        shutdown.request_shutdown();
        let consumer = Arc::new(CollectingConsumer::default());
        let worker_consumer = consumer.clone();

        let handle = thread::spawn(move || {
            mirror_loop(
                IoStream::Stderr,
                Arc::new(Ledger::new()),
                worker_consumer,
                read_fd,
                forward,
                shutdown,
            );
        });
        // Generous bound: this only distinguishes "returns" from "blocks
        // forever on a pipe that never reaches end-of-file".
        let returned = wait_for_join(&handle, Duration::from_secs(10), Duration::from_millis(5));
        drop(write_fd); // unblocks a reader that did block, so the join below ends
        handle.join().expect("mirror loop thread");

        assert!(
            returned,
            "mirror_loop blocked instead of draining and returning"
        );
        assert_eq!(delivered(&consumer), b"pending\n");
    }

    #[test]
    fn wait_for_join_succeeds_for_completed_thread() {
        let handle = thread::spawn(|| {});
        assert!(wait_for_join(
            &handle,
            Duration::from_millis(100),
            Duration::from_millis(5)
        ));
        handle.join().expect("thread join should succeed");
    }

    #[test]
    fn wait_for_join_times_out_for_blocked_thread() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = shutdown.clone();
        let handle = thread::spawn(move || {
            while !worker_shutdown.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(5));
            }
        });

        assert!(
            !wait_for_join(&handle, Duration::from_millis(15), Duration::from_millis(5)),
            "wait_for_join should time out on a stuck thread"
        );

        shutdown.store(true, Ordering::Relaxed);
        handle
            .join()
            .expect("thread join should succeed after shutdown signal");
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
    /// Release pairs with the Acquire in `should_exit`: a reader that observes
    /// the signal also observes that `StreamMirror::shutdown` already restored
    /// the target fd, which is what makes its drain complete.
    fn request_shutdown(&self) {
        self.triggered.store(true, Ordering::Release);
    }

    fn should_exit(&self) -> bool {
        self.triggered.load(Ordering::Acquire)
    }
}

/// Reader side of one mirrored stream.
///
/// Every byte written to the pipe before shutdown must reach the consumer:
/// shutdown is not a reason to discard data. The loop therefore never exits
/// on the shutdown signal alone. It reads (blocking) until end-of-file, which
/// arrives deterministically once `StreamMirror::shutdown` has restored the
/// target fd and so closed the last write end this process holds. When it
/// does observe the signal, it switches the pipe to non-blocking mode and
/// keeps reading until the pipe is empty, so that a write end inherited by a
/// child process (which keeps end-of-file from ever arriving) cannot keep it
/// blocked after the data written before shutdown has been delivered.
///
/// An earlier version checked the signal before each read and returned at
/// once. A reader that had not yet been scheduled, or was between two reads,
/// when shutdown was requested then dropped everything still in the pipe:
/// the bytes reached neither the trace nor the real stream.
fn mirror_loop(
    stream: IoStream,
    ledger: Arc<Ledger>,
    consumer: Arc<dyn IoChunkConsumer>,
    read_fd: OwnedFd,
    forward_fd: OwnedFd,
    shutdown: Arc<ShutdownSignal>,
) {
    let mut buffer = vec![0u8; 8192];
    let mut draining = false;
    loop {
        if !draining && shutdown.should_exit() {
            if let Err(err) = set_nonblocking(read_fd.as_raw_fd()) {
                // Without non-blocking mode the drain still completes on
                // end-of-file; it can only stay blocked behind an inherited
                // write end, which `StreamMirror::shutdown` bounds with its
                // join timeout.
                warn!("fd mirror could not switch {stream} to non-blocking drain: {err}");
            }
            draining = true;
        }

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
            if draining && err.kind() == io::ErrorKind::WouldBlock {
                // The pipe is empty and shutdown has restored the target fd:
                // everything written before shutdown has been delivered.
                break;
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

/// Put `fd` into non-blocking mode (`fcntl(2)` `F_SETFL` with `O_NONBLOCK`,
/// https://man7.org/linux/man-pages/man2/fcntl.2.html), keeping its other
/// status flags.
fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
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
