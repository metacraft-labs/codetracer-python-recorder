//! Logging helpers for runtime tracer callbacks.

use pyo3::Python;

use crate::code_object::CodeObjectWrapper;
use crate::runtime::io_capture::ScopedMuteIoCapture;

/// Emit a debug log entry for tracer callbacks, enriching the message with
/// filename, qualified name, and optional line information when available.
pub fn log_event(py: Python<'_>, code: &CodeObjectWrapper, event: &str, lineno: Option<u32>) {
    let _mute = ScopedMuteIoCapture::new();
    if let Some(line) = lineno {
        match code.filename(py) {
            Ok(filename) => log::debug!("[RuntimeTracer] {event}: {filename}:{line}"),
            Err(_) => log::debug!("[RuntimeTracer] {event}: <unknown>:{line}"),
        }
        return;
    }

    match (code.filename(py), code.qualname(py)) {
        (Ok(filename), Ok(qualname)) => {
            log::debug!("[RuntimeTracer] {event}: {qualname} ({filename})");
        }
        (Ok(filename), Err(_)) => {
            log::debug!("[RuntimeTracer] {event}: ({filename})");
        }
        _ => {
            log::debug!("[RuntimeTracer] {event}");
        }
    }
}
