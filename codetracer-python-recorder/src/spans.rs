//! RS-M5 — the recorder's span-emission surface for Python middleware.
//!
//! A *span* is a bounded, labeled interval of execution — an HTTP request, a
//! process, a test — recorded into the trace container's `spans.dat` stream
//! (spec: `codetracer-specs/Trace-Files/CTFS-Request-Span-Streams.md`).  This
//! module is what `codetracer_python_recorder.middleware.wsgi` /
//! `.asgi` call instead of appending to a `codetracer_spans.jsonl` sidecar, so
//! a recorded request becomes a *(process, thread, step range)* coordinate
//! inside the very container the recorder is writing — which is what lets the
//! Request Panel seek from a row to that request's handler.
//!
//! Three primitives, deliberately small so the middleware stays in Python:
//!
//! * [`span_allocate_id`] — the container-unique, monotonic `span_id`.  Handed
//!   out here rather than by each middleware so a process serving both WSGI and
//!   ASGI traffic (or a future test/process span emitter) cannot mint colliding
//!   ids.
//! * [`span_next_step_index`] — the step index the next recorded event will
//!   occupy, read from the WRITER (never counted recorder-side; see
//!   `NimTraceWriter::next_step_index`).
//! * [`register_span`] — one append to the span stream.
//!
//! All three are no-ops-with-a-signal when no session is active
//! (`register_span` returns `False`, `span_next_step_index` returns `None`), so
//! middleware may be installed unconditionally in an app that is only sometimes
//! recorded.

use std::sync::atomic::{AtomicU64, Ordering};

use pyo3::prelude::*;
use recorder_errors::{enverr, usage, ErrorCode};

use crate::ffi;
use crate::monitoring::{installed_tracer_next_step_index, register_span_on_installed_tracer};
use codetracer_trace_writer_nim::{
    read_span_stream_json as read_span_stream_json_impl, NimTraceReaderHandle, SpanRecord,
    SPAN_STATUS_ERROR, SPAN_STATUS_OK, SPAN_STATUS_UNKNOWN,
};

/// Next `span_id` to hand out.  Span ids must be 1-based and monotonic *within
/// a container*, so [`reset_span_ids`] rewinds this when a session starts.
static NEXT_SPAN_ID: AtomicU64 = AtomicU64::new(1);

/// Rewind the span-id sequence for a fresh session.  Called from
/// `session::start_tracing`: ids are per-container, and a second recording in
/// the same process must start from 1 again.
pub(crate) fn reset_span_ids() {
    NEXT_SPAN_ID.store(1, Ordering::SeqCst);
}

/// Allocate the next container-unique span id.
#[pyfunction]
pub fn span_allocate_id() -> u64 {
    NEXT_SPAN_ID.fetch_add(1, Ordering::SeqCst)
}

/// The step index the next recorded event will occupy — the `start_step` a span
/// opened right now must carry.  `None` when no session is active.
///
/// A span that runs from here to there is `start_step = span_next_step_index()`
/// at entry and `end_step = span_next_step_index() - 1` at exit (clamped to
/// `start_step` when nothing was recorded in between).
#[pyfunction]
pub fn span_next_step_index(py: Python<'_>) -> Option<u64> {
    installed_tracer_next_step_index(py)
}

/// Append one span record to the active recording's span stream.
///
/// Returns `True` when the span was recorded and `False` when no session is
/// active (nothing to record into — not an error).  A session that IS active and
/// cannot store the span raises, so a recorded run never loses requests
/// silently.
///
/// `metadata` is an ordered sequence of `(key, value)` pairs, not a mapping:
/// metadata order is part of the wire contract and consumers render it in
/// emission order.  Emit the well-known `http.*` keys in display order.
///
/// To publish an in-flight request call once with `is_open=True` (and
/// `end_wall_ns` / `end_step` zero), then again on completion with the SAME
/// `span_id`; readers resolve the pair by last-record-wins.
#[pyfunction]
#[pyo3(signature = (
    span_id,
    span_type,
    label,
    *,
    status = SPAN_STATUS_UNKNOWN,
    start_wall_ns = 0,
    end_wall_ns = 0,
    start_step = 0,
    end_step = 0,
    thread_id = 0,
    process_ord = 0,
    parent_span_id = 0,
    is_open = false,
    contiguous_on_one_thread = false,
    shares_timeline = true,
    concurrent_with_siblings = false,
    metadata = Vec::new(),
))]
#[allow(clippy::too_many_arguments)]
pub fn register_span(
    py: Python<'_>,
    span_id: u64,
    span_type: String,
    label: String,
    status: u8,
    start_wall_ns: u64,
    end_wall_ns: u64,
    start_step: u64,
    end_step: u64,
    thread_id: u64,
    process_ord: u64,
    parent_span_id: u64,
    is_open: bool,
    contiguous_on_one_thread: bool,
    shares_timeline: bool,
    concurrent_with_siblings: bool,
    metadata: Vec<(String, String)>,
) -> PyResult<bool> {
    if span_id == 0 {
        return Err(ffi::map_recorder_error(usage!(
            ErrorCode::Unknown,
            "span_id must be >= 1 (0 is the wire encoding of \"no span\")"
        )));
    }
    if status > SPAN_STATUS_ERROR {
        return Err(ffi::map_recorder_error(usage!(
            ErrorCode::Unknown,
            "invalid span status {}; expected {} (unknown), {} (ok) or {} (error)",
            status,
            SPAN_STATUS_UNKNOWN,
            SPAN_STATUS_OK,
            SPAN_STATUS_ERROR
        )));
    }
    let span = SpanRecord {
        span_id,
        parent_span_id,
        is_open,
        is_external: false,
        status,
        start_wall_ns,
        end_wall_ns: if is_open { 0 } else { end_wall_ns },
        process_ord,
        thread_id,
        start_step,
        end_step: if is_open { 0 } else { end_step },
        external_recording: String::new(),
        external_path: String::new(),
        span_type,
        label,
        contiguous_on_one_thread,
        shares_timeline,
        concurrent_with_siblings,
        metadata,
    };
    register_span_on_installed_tracer(py, &span).map_err(|err| {
        ffi::map_recorder_error(
            enverr!(ErrorCode::Io, "failed to record span").with_context("source", err),
        )
    })
}

/// Decode the span stream of a `.ct` container into JSON.
///
/// The READ counterpart of [`register_span`], present so the recorder's own
/// integration tests can assert on the spans they wrote through the CANONICAL
/// Nim decoder (`initSpanStreamReader`, the same one `ct print -f http` uses)
/// rather than a second, test-only decoder that could agree with a writer bug.
///
/// `settled` applies last-record-wins per `span_id` and sorts ascending by
/// `span_id` — what a panel displays.  `settled=False` returns every record in
/// append order, open records included, which is what a test asserting
/// in-flight publication needs.
#[pyfunction]
#[pyo3(signature = (path, settled = true))]
pub fn read_span_stream_json(path: String, settled: bool) -> PyResult<String> {
    read_span_stream_json_impl(std::path::Path::new(&path), settled).map_err(|err| {
        ffi::map_recorder_error(
            enverr!(ErrorCode::Io, "failed to read the container's span stream")
                .with_context("path", path.clone())
                .with_context("source", err.to_string()),
        )
    })
}

/// The number of steps recorded in the `.ct` container at `path`.
///
/// Exposed alongside [`read_span_stream_json`] so a consumer can check that a
/// span's `[start_step, end_step]` really is a coordinate INSIDE this container
/// — the property that distinguishes an inline-bound span from the sidecar rows
/// it replaces.  Read through the canonical Nim reader.
#[pyfunction]
pub fn trace_step_count(path: String) -> PyResult<u64> {
    let reader = NimTraceReaderHandle::open(&path).map_err(|err| {
        ffi::map_recorder_error(
            enverr!(ErrorCode::Io, "failed to open the trace container")
                .with_context("path", path.clone())
                .with_context("source", err.to_string()),
        )
    })?;
    Ok(reader.step_count())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_ids_are_monotonic_and_reset_per_session() {
        reset_span_ids();
        assert_eq!(span_allocate_id(), 1, "span ids are 1-based per container");
        assert_eq!(span_allocate_id(), 2);
        reset_span_ids();
        assert_eq!(
            span_allocate_id(),
            1,
            "a new session restarts the sequence: span ids are container-scoped"
        );
    }
}
