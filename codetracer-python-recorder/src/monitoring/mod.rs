//! Helpers around CPython's `sys.monitoring` API.

use pyo3::prelude::*;
use pyo3::types::PyCFunction;
use std::sync::OnceLock;

pub(crate) mod api;
pub(crate) mod callbacks;
pub(crate) mod install;
pub mod tracer;

pub use api::Tracer;
pub use install::{flush_installed_tracer, install_tracer, uninstall_tracer, update_exit_status};

const MONITORING_TOOL_NAME: &str = "codetracer";

/// Identifier for a monitoring event bit mask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct EventId(pub i32);

#[allow(non_snake_case)]
/// Structured access to CPython's `sys.monitoring.events` values.
#[derive(Clone, Copy, Debug)]
pub struct MonitoringEvents {
    pub BRANCH: EventId,
    pub CALL: EventId,
    pub C_RAISE: EventId,
    pub C_RETURN: EventId,
    pub EXCEPTION_HANDLED: EventId,
    pub INSTRUCTION: EventId,
    pub JUMP: EventId,
    pub LINE: EventId,
    pub PY_RESUME: EventId,
    pub PY_RETURN: EventId,
    pub PY_START: EventId,
    pub PY_THROW: EventId,
    pub PY_UNWIND: EventId,
    pub PY_YIELD: EventId,
    pub RAISE: EventId,
    pub RERAISE: EventId,
    //pub STOP_ITERATION: EventId, //See comment in Tracer trait
}

/// Wrapper returned by `sys.monitoring.use_tool_id`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolId {
    pub id: u8,
}

pub type CallbackFn<'py> = Bound<'py, PyCFunction>;

/// Bit-set describing which events are enabled for a tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventSet(pub i32);

/// Convenience constant representing an empty event mask.
pub const NO_EVENTS: EventSet = EventSet(0);

/// Outcome returned by tracer callbacks to control CPython monitoring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallbackOutcome {
    /// Continue receiving events for the current location.
    Continue,
    /// Disable future events for the current location by returning
    /// `sys.monitoring.DISABLE`.
    DisableLocation,
}

/// Result type shared by tracer callbacks.
pub type CallbackResult = PyResult<CallbackOutcome>;

static MONITORING_EVENTS: OnceLock<MonitoringEvents> = OnceLock::new();

impl EventSet {
    /// Create an empty event mask.
    pub const fn empty() -> Self {
        NO_EVENTS
    }

    /// Return true when the set includes the provided event identifier.
    pub fn contains(&self, ev: &EventId) -> bool {
        (self.0 & ev.0) != 0
    }
}

/// Acquire a monitoring tool id for Codetracer.
pub fn acquire_tool_id(py: Python<'_>) -> PyResult<ToolId> {
    let monitoring = py.import("sys")?.getattr("monitoring")?;
    const FALLBACK_ID: u8 = 5;
    monitoring.call_method1("use_tool_id", (FALLBACK_ID, MONITORING_TOOL_NAME))?;
    Ok(ToolId { id: FALLBACK_ID })
}

/// Load monitoring event identifiers from CPython.
pub fn load_monitoring_events(py: Python<'_>) -> PyResult<MonitoringEvents> {
    let monitoring = py.import("sys")?.getattr("monitoring")?;
    let events = monitoring.getattr("events")?;
    Ok(MonitoringEvents {
        BRANCH: EventId(events.getattr("BRANCH")?.extract()?),
        CALL: EventId(events.getattr("CALL")?.extract()?),
        C_RAISE: EventId(events.getattr("C_RAISE")?.extract()?),
        C_RETURN: EventId(events.getattr("C_RETURN")?.extract()?),
        EXCEPTION_HANDLED: EventId(events.getattr("EXCEPTION_HANDLED")?.extract()?),
        INSTRUCTION: EventId(events.getattr("INSTRUCTION")?.extract()?),
        JUMP: EventId(events.getattr("JUMP")?.extract()?),
        LINE: EventId(events.getattr("LINE")?.extract()?),
        PY_RESUME: EventId(events.getattr("PY_RESUME")?.extract()?),
        PY_RETURN: EventId(events.getattr("PY_RETURN")?.extract()?),
        PY_START: EventId(events.getattr("PY_START")?.extract()?),
        PY_THROW: EventId(events.getattr("PY_THROW")?.extract()?),
        PY_UNWIND: EventId(events.getattr("PY_UNWIND")?.extract()?),
        PY_YIELD: EventId(events.getattr("PY_YIELD")?.extract()?),
        RAISE: EventId(events.getattr("RAISE")?.extract()?),
        RERAISE: EventId(events.getattr("RERAISE")?.extract()?),
        //STOP_ITERATION: EventId(events.getattr("STOP_ITERATION")?.extract()?), //See comment in Tracer trait
    })
}

/// Cache and return the monitoring event structure for the current interpreter.
pub fn monitoring_events(py: Python<'_>) -> PyResult<&'static MonitoringEvents> {
    if let Some(ev) = MONITORING_EVENTS.get() {
        return Ok(ev);
    }
    let ev = load_monitoring_events(py)?;
    let _ = MONITORING_EVENTS.set(ev);
    Ok(MONITORING_EVENTS.get().unwrap())
}

/// Register or unregister a single callback for the provided event.
pub fn register_callback(
    py: Python<'_>,
    tool: &ToolId,
    event: &EventId,
    cb: Option<&CallbackFn<'_>>,
) -> PyResult<()> {
    let monitoring = py.import("sys")?.getattr("monitoring")?;
    match cb {
        Some(cb) => {
            monitoring.call_method("register_callback", (tool.id, event.0, cb), None)?;
        }
        None => {
            monitoring.call_method("register_callback", (tool.id, event.0, py.None()), None)?;
        }
    }
    Ok(())
}

/// Combine multiple event ids into a single bit mask.
pub fn events_union(ids: &[EventId]) -> EventSet {
    let mut bits = 0i32;
    for id in ids {
        bits |= id.0;
    }
    EventSet(bits)
}

/// Enable events for the given tool id.
pub fn set_events(py: Python<'_>, tool: &ToolId, set: EventSet) -> PyResult<()> {
    let monitoring = py.import("sys")?.getattr("monitoring")?;
    monitoring.call_method1("set_events", (tool.id, set.0))?;
    Ok(())
}

/// Release a previously acquired monitoring tool id.
pub fn free_tool_id(py: Python<'_>, tool: &ToolId) -> PyResult<()> {
    let monitoring = py.import("sys")?.getattr("monitoring")?;
    monitoring.call_method1("free_tool_id", (tool.id,))?;
    Ok(())
}
