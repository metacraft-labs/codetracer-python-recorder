//! Correlation markers — the recorder's *thin binding* onto the shared CTFS
//! writer library's marker API.
//!
//! A **correlation marker** declares that a value crossed a boundary: an order
//! id put on a queue here and taken off it there, a request id sent by one
//! service and received by another.  Two markers sharing a `boundary_id` with
//! opposing directions form a pair, and that pair is what lets CodeTracer walk
//! an origin chain out of one recording and into another.
//!
//! Spec: `codetracer-specs/Testing/CTFS-Correlation-Marker-Contract.md` (§10,
//! §11a) and `codetracer-specs/GUI/Debugging-Features/Correlation-Markers.md`
//! §2.4 "Programmatic API", which owns the user-facing spelling.
//!
//! **Nothing here decides what a marker *is*.**  Payload construction, field
//! naming and `corrmark.ns` population all live once in the shared writer
//! library (`codetracer-trace-format-nim`), because a marker whose field names
//! drift is not degraded but *unreadable* — and nothing reports an error when
//! that happens.  This module marshals arguments and forwards.  §11a.1.
//!
//! Four properties of this module are load-bearing rather than stylistic:
//!
//! * **The numeric-id call is primary.**  [`ensure_marker_id`] interns a
//!   boundary label once, mirroring `ensure_path_id`; a caller hoists it out of
//!   its hot path and passes the integer to [`mark_correlation_by_id`], so the
//!   per-crossing call does no string lookup.  The string form
//!   ([`mark_correlation`]) is a wrapper over the numeric one, never the
//!   reverse — if it were primary every recorder would grow its own label cache
//!   and they would drift.  §11a.4.
//! * **Every host→`str` conversion has already happened when we are called.**
//!   PyO3 renders the arguments before the function body runs, so no host code
//!   can execute while the writer lock is held.  A host exception raised under
//!   that lock could unwind past the guard and wedge the process permanently.
//!   §11a.5.
//! * **Strings travel as `&str`** — pointer plus length — all the way to the C
//!   ABI.  Never a `CString`: a Python `str` may legally contain NUL, and
//!   NUL-terminating it would silently truncate the marker.  §11a.5.
//! * **No step is minted.**  A marker attaches to the enclosing step.  Minting
//!   one would insert an exec-stream event that no user code executed and shift
//!   every later step index — the very indices spans' `start_step` / `end_step`
//!   and every other step-addressed coordinate are measured in.  §11a.6.
//!
//! Every entry point is a no-op *with a signal* when no session is active
//! ([`ensure_marker_id`] returns `None`, the rest return `False`), so a library
//! may declare its boundaries unconditionally and still be importable in a
//! process that is not being recorded.  That is a contract, not an error path.

use pyo3::prelude::*;
use recorder_errors::{enverr, ErrorCode};

use crate::ffi;
use crate::monitoring::{
    ensure_marker_id_on_installed_tracer, mark_correlation_on_installed_tracer,
    mark_span_coverage_on_installed_tracer, CorrelationMarker,
};

/// Turn `Option<&str>` into the `""` the shared writer reads as "use the
/// default", so the absent case is spelled once instead of at every call site.
fn or_empty(value: Option<&str>) -> &str {
    value.unwrap_or("")
}

/// Map a marker failure from an ACTIVE session into a recorder error.
///
/// Only reached when a session exists and the writer refused the marker — the
/// "no session" case never gets here, because it is a legitimate no-op.
fn marker_error(operation: &'static str, err: String) -> PyErr {
    ffi::map_recorder_error(
        enverr!(ErrorCode::Io, "failed to record a correlation marker")
            .with_context("operation", operation)
            .with_context("source", err),
    )
}

/// Intern a correlation-marker boundary label and return its id.
///
/// `None` means no session is active, which callers must distinguish from
/// marker id `0`.  Hoist this out of a hot path — once per boundary, not once
/// per crossing — and pass the result to [`mark_correlation_by_id`].
#[pyfunction]
pub fn ensure_marker_id(py: Python<'_>, label: &str) -> PyResult<Option<u64>> {
    ensure_marker_id_on_installed_tracer(py, label)
        .map_err(|err| marker_error("ensure_marker_id", err))
}

/// Declare a boundary crossing against an already-interned label id.
///
/// THE HOT-PATH ENTRY POINT.  Returns `True` when the marker was recorded and
/// `False` when no session is active (nothing to record into — not an error).
/// A session that IS active and cannot store the marker raises, so a recorded
/// run never loses a boundary crossing silently.
///
/// `key_value` / `show_value` are already-rendered UTF-8; see the module docs
/// for why this API cannot accept an opaque value and render it itself.
///
/// `key_text` / `show_text` are the NAMES those values were read under.
/// `show_text` is load-bearing rather than cosmetic: a cross-process origin
/// chain resumes its walk on that name in the sending recording, so a marker
/// that drops it is visible with its history unreachable.  Pass `None` for the
/// library defaults.
#[pyfunction]
#[pyo3(signature = (
    marker_id,
    boundary_label,
    direction,
    key_value,
    show_value = None,
    description = None,
    key_text = None,
    show_text = None,
))]
#[allow(clippy::too_many_arguments)]
pub fn mark_correlation_by_id(
    py: Python<'_>,
    marker_id: u64,
    boundary_label: &str,
    direction: &str,
    key_value: &str,
    show_value: Option<&str>,
    description: Option<&str>,
    key_text: Option<&str>,
    show_text: Option<&str>,
) -> PyResult<bool> {
    let marker = CorrelationMarker {
        marker_id: Some(marker_id),
        boundary: boundary_label,
        direction,
        key_value,
        show_value: or_empty(show_value),
        description: or_empty(description),
        key_text: or_empty(key_text),
        show_text: or_empty(show_text),
    };
    mark_correlation_on_installed_tracer(py, &marker)
        .map_err(|err| marker_error("mark_correlation_by_id", err))
}

/// Declare a boundary crossing by label — a wrapper that interns and forwards.
///
/// Convenient for a cold path.  A hot path should hoist [`ensure_marker_id`]
/// and call [`mark_correlation_by_id`] instead; see §11a.4.
#[pyfunction]
#[pyo3(signature = (
    direction,
    boundary,
    key_value,
    show_value = None,
    description = None,
    key_text = None,
    show_text = None,
))]
#[allow(clippy::too_many_arguments)]
pub fn mark_correlation(
    py: Python<'_>,
    direction: &str,
    boundary: &str,
    key_value: &str,
    show_value: Option<&str>,
    description: Option<&str>,
    key_text: Option<&str>,
    show_text: Option<&str>,
) -> PyResult<bool> {
    let marker = CorrelationMarker {
        marker_id: None,
        boundary,
        direction,
        key_value,
        show_value: or_empty(show_value),
        description: or_empty(description),
        key_text: or_empty(key_text),
        show_text: or_empty(show_text),
    };
    mark_correlation_on_installed_tracer(py, &marker)
        .map_err(|err| marker_error("mark_correlation", err))
}

/// Declare that this recording covers a distributed-trace span.
///
/// The ids arrive as hex because that is the form every OTel API hands out
/// (`format_trace_id` / `format_span_id`): 32 hex characters for `trace_id`,
/// 16 for `span_id`, either case.  The hex→bytes conversion is done by the
/// SHARED library so it has one implementation rather than one per recorder —
/// the index keys on the wire bytes, and an index keyed on a hex rendering
/// would be present, correct-looking, permanently unqueryable, and silent.
///
/// Unlike [`mark_correlation`] this mints no `MarkerPayload` and no IO event:
/// a span-coverage marker has no send/recv sense and no pairing domain, so
/// forcing it into one would make the pairing index try to pair spans with
/// each other (contract §10.2).
///
/// Returns `False` when no session is active.
#[pyfunction]
pub fn mark_span_coverage(
    py: Python<'_>,
    trace_id_hex: &str,
    span_id_hex: &str,
    wall_time_unix_ns: u64,
    monotonic_time_ns: u64,
) -> PyResult<bool> {
    mark_span_coverage_on_installed_tracer(
        py,
        trace_id_hex,
        span_id_hex,
        wall_time_unix_ns,
        monotonic_time_ns,
    )
    .map_err(|err| marker_error("mark_span_coverage", err))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_optional_strings_become_the_librarys_default_sentinel() {
        // The shared writer reads `""` as "use your default" for every optional
        // field.  Spelling that mapping once here is what keeps `None` from
        // reaching the C ABI as the literal text "None".
        assert_eq!(or_empty(None), "");
        assert_eq!(or_empty(Some("")), "");
        assert_eq!(or_empty(Some("order.id")), "order.id");
    }

    #[test]
    fn a_marker_carries_no_step_coordinate() {
        // Guards §11a.6 structurally: there is no field on the struct through
        // which this binding could attribute a marker to a step of its own, so
        // a marker can only ever attach to the enclosing one.
        let marker = CorrelationMarker {
            marker_id: Some(7),
            boundary: "order-processing",
            direction: "send",
            key_value: "abc",
            show_value: "",
            description: "",
            key_text: "",
            show_text: "",
        };
        assert_eq!(marker.marker_id, Some(7));
        assert_eq!(marker.boundary, "order-processing");
    }
}
