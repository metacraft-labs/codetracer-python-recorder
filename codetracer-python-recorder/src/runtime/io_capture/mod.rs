#![allow(dead_code)]

//! Cross-platform IO capture workers for stdout, stderr, and stdin.

use std::thread::JoinHandle;
use std::time::Instant;

use crossbeam_channel::{bounded, Receiver, Sender};

use recorder_errors::{bug, enverr, ErrorCode, RecorderResult};

use crate::errors::Result;

use super::IoDrain;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as platform;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

const CHANNEL_CAPACITY: usize = 1024;

pub type IoChunkReceiver = Receiver<IoChunk>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamKind {
    Stdout,
    Stderr,
    Stdin,
}

impl StreamKind {
    pub fn as_str(self) -> &'static str {
        match self {
            StreamKind::Stdout => "stdout",
            StreamKind::Stderr => "stderr",
            StreamKind::Stdin => "stdin",
        }
    }
}

#[allow(dead_code)]
#[derive(Debug)]
pub struct IoChunk {
    pub stream: StreamKind,
    pub timestamp: Instant,
    pub bytes: Vec<u8>,
    pub producer_thread: std::thread::ThreadId,
}

type WorkerHandle = JoinHandle<Result<()>>;

pub struct IoCapture {
    receiver: Option<IoChunkReceiver>,
    workers: Vec<WorkerHandle>,
    platform: Option<platform::Controller>,
}

impl IoCapture {
    pub fn start() -> Result<Self> {
        let (tx, rx) = bounded(CHANNEL_CAPACITY);
        let (controller, workers) = platform::start(tx)?;
        Ok(Self {
            receiver: Some(rx),
            workers,
            platform: Some(controller),
        })
    }

    pub fn take_receiver(&mut self) -> Option<IoChunkReceiver> {
        self.receiver.take()
    }

    pub fn shutdown(&mut self) -> Result<()> {
        if let Some(mut controller) = self.platform.take() {
            controller.restore()?;
        }
        self.join_workers()
    }

    fn join_workers(&mut self) -> Result<()> {
        for handle in self.workers.drain(..) {
            match handle.join() {
                Ok(Ok(())) => {}
                Ok(Err(err)) => return Err(err),
                Err(panic) => {
                    let message = if let Some(msg) = panic.downcast_ref::<&'static str>() {
                        msg.to_string()
                    } else if let Some(msg) = panic.downcast_ref::<String>() {
                        msg.clone()
                    } else {
                        "unknown panic".to_string()
                    };
                    return Err(bug!(ErrorCode::Unknown, "IO capture worker panicked")
                        .with_context("details", message));
                }
            }
        }
        Ok(())
    }
}

impl IoDrain for IoCapture {
    fn drain(&mut self, _py: pyo3::Python<'_>) -> RecorderResult<()> {
        self.shutdown()
    }
}

impl Drop for IoCapture {
    fn drop(&mut self) {
        if self.platform.is_some() {
            if let Err(err) = self.shutdown() {
                log::error!("failed to shutdown IO capture cleanly: {}", err);
            }
        }
    }
}

pub(super) fn spawn_reader_thread<F>(name: &'static str, worker: F) -> Result<WorkerHandle>
where
    F: FnOnce() -> Result<()> + Send + 'static,
{
    std::thread::Builder::new()
        .name(format!("io-capture-{}", name))
        .spawn(worker)
        .map_err(|err| {
            enverr!(
                recorder_errors::ErrorCode::Io,
                "failed to spawn IO capture worker thread"
            )
            .with_context("thread", name)
            .with_context("error", err.to_string())
        })
}

fn build_chunk(stream: StreamKind, bytes: Vec<u8>) -> IoChunk {
    IoChunk {
        stream,
        timestamp: Instant::now(),
        producer_thread: std::thread::current().id(),
        bytes,
    }
}

fn send_chunk(sender: &Sender<IoChunk>, chunk: IoChunk) {
    let stream = chunk.stream;
    if let Err(err) = sender.send(chunk) {
        log::warn!(
            "dropping IO chunk for {} because receiver closed: {}",
            stream.as_str(),
            err
        );
    }
}

pub(super) fn send_buffer(sender: &Sender<IoChunk>, stream: StreamKind, buffer: &[u8]) {
    if buffer.is_empty() {
        return;
    }
    send_chunk(sender, build_chunk(stream, buffer.to_vec()));
}

#[cfg(test)]
mod tests {
    use super::*;

    use once_cell::sync::Lazy;
    use std::io::{Read, Write};
    use std::sync::Mutex;
    use std::time::Duration;

    static STDIO_GUARD: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    #[cfg(unix)]
    use os_pipe::pipe;
    #[cfg(unix)]
    use std::os::fd::AsRawFd;

    #[cfg(unix)]
    #[test]
    fn captures_stdout_and_preserves_passthrough() {
        let _guard = STDIO_GUARD.lock().unwrap();

        let (mut reader, writer) = pipe().expect("pipe");
        let original_stdout = unsafe { libc::dup(libc::STDOUT_FILENO) };
        assert!(original_stdout >= 0);
        unsafe {
            libc::dup2(writer.as_raw_fd(), libc::STDOUT_FILENO);
        }
        drop(writer);

        let mut capture = IoCapture::start().expect("start capture");
        let receiver = capture.take_receiver().expect("receiver available");

        write!(std::io::stdout(), "hello").expect("write stdout");
        std::io::stdout().flush().expect("flush stdout");

        let chunk = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("chunk arrived");
        assert_eq!(chunk.stream, StreamKind::Stdout);
        assert_eq!(chunk.bytes, b"hello");

        let mut mirror_buf = [0u8; 5];
        let read = reader.read(&mut mirror_buf).expect("read mirror output");
        assert_eq!(&mirror_buf[..read], b"hello");

        capture.shutdown().expect("shutdown capture");
        unsafe {
            libc::dup2(original_stdout, libc::STDOUT_FILENO);
            libc::close(original_stdout);
        }
    }

    #[cfg(unix)]
    #[test]
    fn captures_stdin_and_forwards_bytes() {
        let _guard = STDIO_GUARD.lock().unwrap();

        let (reader, mut writer) = pipe().expect("pipe");
        let original_stdin = unsafe { libc::dup(libc::STDIN_FILENO) };
        assert!(original_stdin >= 0);
        unsafe {
            libc::dup2(reader.as_raw_fd(), libc::STDIN_FILENO);
        }
        drop(reader);

        let mut capture = IoCapture::start().expect("start capture");
        let receiver = capture.take_receiver().expect("receiver available");

        let read_thread = std::thread::spawn(|| {
            let mut buf = [0u8; 5];
            std::io::stdin()
                .read_exact(&mut buf)
                .expect("read from stdin through capture");
            buf
        });

        writer
            .write_all(b"world")
            .expect("write to synthetic stdin");
        writer.flush().expect("flush synthetic stdin");
        drop(writer);

        let forwarded = read_thread.join().expect("join reader");
        assert_eq!(&forwarded, b"world");

        let chunk = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("chunk arrived");
        assert_eq!(chunk.stream, StreamKind::Stdin);
        assert_eq!(chunk.bytes, b"world");

        capture.shutdown().expect("shutdown capture");
        unsafe {
            libc::dup2(original_stdin, libc::STDIN_FILENO);
            libc::close(original_stdin);
        }
    }

    #[cfg(windows)]
    #[test]
    fn captures_stdout_on_windows() {
        use std::fs::OpenOptions;
        use std::os::windows::io::IntoRawHandle;
        use tempfile::NamedTempFile;

        let _guard = STDIO_GUARD.lock().unwrap();

        let tmp = NamedTempFile::new().expect("temp file");
        let path = tmp.path().to_path_buf();
        let file = tmp.into_file();
        let handle = file.into_raw_handle();
        let fd = unsafe { libc::_open_osfhandle(handle as isize, libc::_O_BINARY | libc::_O_RDWR) };
        assert!(fd >= 0);

        let original_stdout = unsafe { libc::_dup(1) };
        assert!(original_stdout >= 0);
        unsafe {
            libc::_dup2(fd, 1);
        }

        let mut capture = IoCapture::start().expect("start capture");
        let receiver = capture.take_receiver().expect("receiver available");

        write!(std::io::stdout(), "hiwin").expect("write stdout");
        std::io::stdout().flush().expect("flush stdout");

        let chunk = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("chunk arrived");
        assert_eq!(chunk.stream, StreamKind::Stdout);
        assert_eq!(chunk.bytes, b"hiwin");

        capture.shutdown().expect("shutdown capture");
        unsafe {
            libc::_dup2(original_stdout, 1);
            libc::_close(original_stdout);
            libc::_close(fd);
        }

        let mut stored = Vec::new();
        let mut reopened = OpenOptions::new()
            .read(true)
            .open(path)
            .expect("re-open temp file");
        reopened
            .read_to_end(&mut stored)
            .expect("read captured file");
        assert_eq!(stored, b"hiwin");
    }
}
