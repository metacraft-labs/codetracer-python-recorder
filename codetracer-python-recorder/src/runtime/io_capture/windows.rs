use std::io::ErrorKind;

use crossbeam_channel::Sender;

use recorder_errors::{enverr, ErrorCode};

use crate::errors::Result;

use super::{send_buffer, spawn_reader_thread, IoChunk, StreamKind, WorkerHandle};

const READ_BUFFER_SIZE: usize = 8192;
const PIPE_BUFFER_SIZE: u32 = 8192;

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
            restore_descriptor(input.saved_fd, StreamKind::Stdin)?;
            close_fd(input.source_fd);
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
    saved_fd: i32,
}

struct InputGuard {
    saved_fd: i32,
    source_fd: i32,
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
    let (reader_fd, writer_fd) = new_pipe(stream)?;

    replace_fd(writer_fd, fd, stream)?;
    close_fd(writer_fd);

    let tx = sender.clone();
    let worker = match spawn_reader_thread(stream.as_str(), move || {
        run_output_reader(stream, reader_fd, mirror_fd, tx)
    }) {
        Ok(worker) => worker,
        Err(err) => {
            close_fd(reader_fd);
            close_fd(mirror_fd);
            if unsafe { libc::_dup2(saved_fd, fd) } < 0 {
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
    let (pipe_reader_fd, pipe_writer_fd) = new_pipe(StreamKind::Stdin)?;

    replace_fd(pipe_reader_fd, fd, StreamKind::Stdin)?;
    close_fd(pipe_reader_fd);

    let tx = sender.clone();
    let worker = match spawn_reader_thread("stdin", move || {
        run_input_reader(source_fd, pipe_writer_fd, tx)
    }) {
        Ok(worker) => worker,
        Err(err) => {
            close_fd(pipe_writer_fd);
            close_fd(source_fd);
            if unsafe { libc::_dup2(saved_fd, fd) } < 0 {
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
            source_fd,
        },
        worker,
    ))
}

fn run_output_reader(
    stream: StreamKind,
    reader_fd: i32,
    mirror_fd: i32,
    sender: Sender<IoChunk>,
) -> Result<()> {
    let mut buffer = [0u8; READ_BUFFER_SIZE];

    loop {
        let read = unsafe {
            libc::_read(
                reader_fd,
                buffer.as_mut_ptr() as *mut libc::c_void,
                READ_BUFFER_SIZE as i32,
            )
        };
        if read == 0 {
            break;
        }
        if read < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == ErrorKind::Interrupted {
                continue;
            }
            close_fd(reader_fd);
            close_fd(mirror_fd);
            return Err(enverr!(ErrorCode::Io, "failed to read captured output")
                .with_context("stream", stream.as_str())
                .with_context("error", err.to_string()));
        }

        let count = read as usize;
        send_buffer(&sender, stream, &buffer[..count]);
        let written =
            unsafe { libc::_write(mirror_fd, buffer.as_ptr() as *const libc::c_void, read) };
        if written < 0 {
            let err = std::io::Error::last_os_error();
            close_fd(reader_fd);
            close_fd(mirror_fd);
            return Err(enverr!(ErrorCode::Io, "failed to mirror captured output")
                .with_context("stream", stream.as_str())
                .with_context("error", err.to_string()));
        }
    }

    close_fd(reader_fd);
    close_fd(mirror_fd);
    Ok(())
}

fn run_input_reader(source_fd: i32, writer_fd: i32, sender: Sender<IoChunk>) -> Result<()> {
    let mut buffer = [0u8; READ_BUFFER_SIZE];

    loop {
        let read = unsafe {
            libc::_read(
                source_fd,
                buffer.as_mut_ptr() as *mut libc::c_void,
                READ_BUFFER_SIZE as i32,
            )
        };

        if read == 0 {
            break;
        }

        if read < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == ErrorKind::Interrupted {
                continue;
            }
            // When the manager closes source_fd to request shutdown, errno becomes EBADF.
            if err.raw_os_error() == Some(libc::EBADF) {
                break;
            }
            close_fd(writer_fd);
            return Err(enverr!(ErrorCode::Io, "failed to read stdin for capture")
                .with_context("error", err.to_string()));
        }

        let count = read as usize;
        send_buffer(&sender, StreamKind::Stdin, &buffer[..count]);
        let written =
            unsafe { libc::_write(writer_fd, buffer.as_ptr() as *const libc::c_void, read) };
        if written < 0 {
            let err = std::io::Error::last_os_error();
            close_fd(writer_fd);
            return Err(enverr!(ErrorCode::Io, "failed to forward stdin chunk")
                .with_context("error", err.to_string()));
        }
    }

    close_fd(writer_fd);
    Ok(())
}

fn dup_fd(fd: i32, stream: StreamKind) -> Result<i32> {
    let result = unsafe { libc::_dup(fd) };
    if result < 0 {
        let err = std::io::Error::last_os_error();
        return Err(
            enverr!(ErrorCode::Io, "failed to duplicate file descriptor")
                .with_context("stream", stream.as_str())
                .with_context("error", err.to_string()),
        );
    }
    Ok(result)
}

fn new_pipe(stream: StreamKind) -> Result<(i32, i32)> {
    let mut fds = [0i32; 2];
    if unsafe {
        libc::_pipe(
            fds.as_mut_ptr(),
            PIPE_BUFFER_SIZE,
            libc::_O_BINARY | libc::_O_NOINHERIT,
        )
    } < 0
    {
        let err = std::io::Error::last_os_error();
        return Err(
            enverr!(ErrorCode::Io, "failed to allocate pipe for capture")
                .with_context("stream", stream.as_str())
                .with_context("error", err.to_string()),
        );
    }
    Ok((fds[0], fds[1]))
}

fn replace_fd(source_fd: i32, target_fd: i32, stream: StreamKind) -> Result<()> {
    if unsafe { libc::_dup2(source_fd, target_fd) } < 0 {
        let err = std::io::Error::last_os_error();
        return Err(enverr!(ErrorCode::Io, "failed to redirect stream to pipe")
            .with_context("stream", stream.as_str())
            .with_context("error", err.to_string()));
    }
    Ok(())
}

fn restore_descriptor(saved_fd: i32, stream: StreamKind) -> Result<()> {
    let target_fd = std_fd(stream);
    if unsafe { libc::_dup2(saved_fd, target_fd) } < 0 {
        let err = std::io::Error::last_os_error();
        return Err(
            enverr!(ErrorCode::Io, "failed to restore descriptor after capture")
                .with_context("stream", stream.as_str())
                .with_context("error", err.to_string()),
        );
    }
    close_fd(saved_fd);
    Ok(())
}

fn close_fd(fd: i32) {
    if fd >= 0 {
        unsafe {
            libc::_close(fd);
        }
    }
}

fn std_fd(stream: StreamKind) -> i32 {
    match stream {
        StreamKind::Stdout => 1,
        StreamKind::Stderr => 2,
        StreamKind::Stdin => 0,
    }
}
