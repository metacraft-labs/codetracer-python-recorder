#![allow(dead_code)]

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};

use crossbeam_channel::Sender;

use recorder_errors::{enverr, ErrorCode};

use crate::errors::Result;

use super::{send_buffer, spawn_reader_thread, IoChunk, StreamKind, WorkerHandle};

const READ_BUFFER_SIZE: usize = 8192;

pub(super) struct Controller {
    outputs: Vec<OutputGuard>,
    input: Option<InputGuard>,
}

impl Controller {
    pub fn restore(&mut self) -> Result<()> {
        for guard in self.outputs.drain(..) {
            restore_descriptor(guard.saved_fd, guard.stream)?;
        }

        if let Some(input) = self.input.take() {
            input.restore()?;
        }

        Ok(())
    }

    fn rollback(&mut self) {
        if let Err(err) = self.restore() {
            log::error!("IO capture rollback failed: {}", err);
        }
    }
}

struct OutputGuard {
    stream: StreamKind,
    saved_fd: OwnedFd,
}

struct InputGuard {
    saved_fd: OwnedFd,
    shutdown_writer: OwnedFd,
}

impl InputGuard {
    fn restore(self) -> Result<()> {
        let shutdown_writer = self.shutdown_writer;
        restore_descriptor(self.saved_fd, StreamKind::Stdin)?;
        signal_shutdown_writer(shutdown_writer)
    }
}

fn signal_shutdown_writer(writer: OwnedFd) -> Result<()> {
    let fd_ref = writer.as_raw_fd();
    let byte = [1u8; 1];
    let result = unsafe { libc::write(fd_ref, byte.as_ptr() as *const libc::c_void, byte.len()) };
    let fd = writer.into_raw_fd();
    let _ = unsafe { libc::close(fd) };
    if result < 0 {
        let errno = std::io::Error::last_os_error();
        return Err(
            enverr!(ErrorCode::Io, "failed to signal stdin capture shutdown")
                .with_context("error", errno.to_string()),
        );
    }
    Ok(())
}

pub(super) fn start(sender: Sender<IoChunk>) -> Result<(Controller, Vec<WorkerHandle>)> {
    let mut controller = Controller {
        outputs: Vec::new(),
        input: None,
    };

    let mut workers: Vec<WorkerHandle> = Vec::new();

    for stream in [StreamKind::Stdout, StreamKind::Stderr] {
        match start_output_stream(stream, &sender) {
            Ok((guard, worker)) => {
                controller.outputs.push(guard);
                workers.push(worker);
            }
            Err(err) => {
                controller.rollback();
                return Err(err);
            }
        }
    }

    match start_input_stream(&sender) {
        Ok((guard, worker)) => {
            controller.input = Some(guard);
            workers.push(worker);
        }
        Err(err) => {
            controller.rollback();
            return Err(err);
        }
    }

    Ok((controller, workers))
}

fn start_output_stream(
    stream: StreamKind,
    sender: &Sender<IoChunk>,
) -> Result<(OutputGuard, WorkerHandle)> {
    let fd = std_fd(stream);
    let saved_fd = dup_fd(fd, stream)?;
    let mirror_fd = dup_fd(fd, stream)?;
    let (reader, writer) = new_pipe()?;

    replace_fd(writer.as_raw_fd(), fd, stream)?;

    // Drop the extra writer descriptor; stdout/stderr already point to it via dup2.
    drop(writer);

    let reader_fd = reader.into_raw_fd();
    let mirror_fd_raw = mirror_fd.into_raw_fd();
    let tx = sender.clone();

    let worker = match spawn_reader_thread(stream.as_str(), move || {
        run_output_reader(stream, reader_fd, mirror_fd_raw, tx)
    }) {
        Ok(worker) => worker,
        Err(err) => {
            close_fd(reader_fd);
            close_fd(mirror_fd_raw);
            let target_fd = std_fd(stream);
            if unsafe { libc::dup2(saved_fd.as_raw_fd(), target_fd) } < 0 {
                log::error!(
                    "failed to restore {} after spawn failure: {}",
                    stream.as_str(),
                    std::io::Error::last_os_error()
                );
            }
            return Err(err);
        }
    };

    Ok((OutputGuard { stream, saved_fd }, worker))
}

fn start_input_stream(sender: &Sender<IoChunk>) -> Result<(InputGuard, WorkerHandle)> {
    let fd = std_fd(StreamKind::Stdin);
    let saved_fd = dup_fd(fd, StreamKind::Stdin)?;
    let source_fd = dup_fd(fd, StreamKind::Stdin)?;
    let (pipe_reader, pipe_writer) = new_pipe()?;
    let (shutdown_reader, shutdown_writer) = new_pipe()?;

    replace_fd(pipe_reader.as_raw_fd(), fd, StreamKind::Stdin)?;
    drop(pipe_reader);

    let writer_fd = pipe_writer.into_raw_fd();
    let source_fd_raw = source_fd.into_raw_fd();
    let shutdown_reader_fd = shutdown_reader.into_raw_fd();
    let tx = sender.clone();

    let worker = match spawn_reader_thread("stdin", move || {
        run_input_reader(source_fd_raw, writer_fd, shutdown_reader_fd, tx)
    }) {
        Ok(worker) => worker,
        Err(err) => {
            close_fd(writer_fd);
            close_fd(source_fd_raw);
            close_fd(shutdown_reader_fd);
            let target_fd = std_fd(StreamKind::Stdin);
            if unsafe { libc::dup2(saved_fd.as_raw_fd(), target_fd) } < 0 {
                log::error!(
                    "failed to restore stdin after spawn failure: {}",
                    std::io::Error::last_os_error()
                );
            }
            return Err(err);
        }
    };

    Ok((
        InputGuard {
            saved_fd,
            shutdown_writer,
        },
        worker,
    ))
}

fn run_output_reader(
    stream: StreamKind,
    reader_fd: RawFd,
    mirror_fd: RawFd,
    sender: Sender<IoChunk>,
) -> Result<()> {
    let mut reader = unsafe { File::from_raw_fd(reader_fd) };
    let mut mirror = unsafe { File::from_raw_fd(mirror_fd) };
    let mut buffer = [0u8; READ_BUFFER_SIZE];

    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                send_buffer(&sender, stream, &buffer[..count]);
                if let Err(err) = mirror.write_all(&buffer[..count]) {
                    return Err(enverr!(ErrorCode::Io, "failed to mirror captured output")
                        .with_context("stream", stream.as_str())
                        .with_context("error", err.to_string()));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => {
                return Err(enverr!(ErrorCode::Io, "failed to read captured output")
                    .with_context("stream", stream.as_str())
                    .with_context("error", err.to_string()));
            }
        }
    }

    Ok(())
}

fn run_input_reader(
    source_fd: RawFd,
    writer_fd: RawFd,
    shutdown_fd: RawFd,
    sender: Sender<IoChunk>,
) -> Result<()> {
    let mut source = unsafe { File::from_raw_fd(source_fd) };
    let mut writer = unsafe { File::from_raw_fd(writer_fd) };
    let mut buffer = [0u8; READ_BUFFER_SIZE];

    loop {
        let mut fds = [
            libc::pollfd {
                fd: source_fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: shutdown_fd,
                events: libc::POLLIN,
                revents: 0,
            },
        ];

        let rc = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
        if rc < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(enverr!(ErrorCode::Io, "stdin capture poll failed")
                .with_context("error", err.to_string()));
        }

        if fds[1].revents & libc::POLLIN != 0 {
            break;
        }

        if fds[0].revents & libc::POLLIN != 0 {
            match source.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    send_buffer(&sender, StreamKind::Stdin, &buffer[..count]);
                    if let Err(err) = writer.write_all(&buffer[..count]) {
                        return Err(enverr!(ErrorCode::Io, "failed to forward stdin chunk")
                            .with_context("error", err.to_string()));
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(err) => {
                    return Err(enverr!(ErrorCode::Io, "failed to read stdin for capture")
                        .with_context("error", err.to_string()));
                }
            }
        }
    }

    // Drain shutdown notification byte if present and close control fd.
    let mut temp = [0u8; 1];
    let _ = unsafe { libc::read(shutdown_fd, temp.as_mut_ptr() as *mut _, 1) };
    let _ = unsafe { libc::close(shutdown_fd) };
    let _ = writer.flush();

    Ok(())
}

fn close_fd(fd: RawFd) {
    if fd >= 0 {
        let _ = unsafe { libc::close(fd) };
    }
}

fn dup_fd(fd: RawFd, stream: StreamKind) -> Result<OwnedFd> {
    let result = unsafe { libc::dup(fd) };
    if result < 0 {
        let err = std::io::Error::last_os_error();
        return Err(
            enverr!(ErrorCode::Io, "failed to duplicate file descriptor")
                .with_context("stream", stream.as_str())
                .with_context("error", err.to_string()),
        );
    }
    // SAFETY: `result` is a freshly duplicated descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(result) })
}

fn new_pipe() -> Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
        let err = std::io::Error::last_os_error();
        return Err(
            enverr!(ErrorCode::Io, "failed to create pipe for IO capture")
                .with_context("error", err.to_string()),
        );
    }
    // SAFETY: pipe returns two valid descriptors on success.
    let reader = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let writer = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    Ok((reader, writer))
}

fn replace_fd(source_fd: RawFd, target_fd: RawFd, stream: StreamKind) -> Result<()> {
    if unsafe { libc::dup2(source_fd, target_fd) } < 0 {
        let err = std::io::Error::last_os_error();
        return Err(enverr!(ErrorCode::Io, "failed to redirect stream to pipe")
            .with_context("stream", stream.as_str())
            .with_context("error", err.to_string()));
    }
    Ok(())
}

fn restore_descriptor(saved: OwnedFd, stream: StreamKind) -> Result<()> {
    let target_fd = std_fd(stream);
    if unsafe { libc::dup2(saved.as_raw_fd(), target_fd) } < 0 {
        let err = std::io::Error::last_os_error();
        return Err(
            enverr!(ErrorCode::Io, "failed to restore descriptor after capture")
                .with_context("stream", stream.as_str())
                .with_context("error", err.to_string()),
        );
    }
    // Drop saved descriptor to avoid leaks.
    drop(saved);
    Ok(())
}

fn std_fd(stream: StreamKind) -> RawFd {
    match stream {
        StreamKind::Stdout => libc::STDOUT_FILENO,
        StreamKind::Stderr => libc::STDERR_FILENO,
        StreamKind::Stdin => libc::STDIN_FILENO,
    }
}
