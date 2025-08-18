use std::sync::{Mutex, OnceLock};
use pyo3::{
    exceptions::PyRuntimeError,
    prelude::*,
    types::{PyAny, PyCFunction, PyModule},
};

const MONITORING_TOOL_NAME: &str = "codetracer";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct EventId(pub i32);

#[allow(non_snake_case)]
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
    pub STOP_ITERATION: EventId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolId {
    pub id: u8,
}

pub type CallbackFn<'py> = Bound<'py, PyCFunction>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventSet(pub i32);

pub const NO_EVENTS: EventSet = EventSet(0);

impl EventSet {
    pub const fn empty() -> Self {
        NO_EVENTS
    }
    pub fn contains(&self, ev: &EventId) -> bool {
        (self.0 & ev.0) != 0
    }
}

pub fn acquire_tool_id(py: Python<'_>) -> PyResult<ToolId> {
    let monitoring = py.import("sys")?.getattr("monitoring")?;
    const FALLBACK_ID: u8 = 5;
    monitoring.call_method1("use_tool_id", (FALLBACK_ID, MONITORING_TOOL_NAME))?;
    Ok(ToolId { id: FALLBACK_ID })
}

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
        STOP_ITERATION: EventId(events.getattr("STOP_ITERATION")?.extract()?),
    })
}

static MONITORING_EVENTS: OnceLock<MonitoringEvents> = OnceLock::new();

pub fn monitoring_events(py: Python<'_>) -> PyResult<&'static MonitoringEvents> {
    if let Some(ev) = MONITORING_EVENTS.get() {
        return Ok(ev);
    }
    let ev = load_monitoring_events(py)?;
    let _ = MONITORING_EVENTS.set(ev);
    Ok(MONITORING_EVENTS.get().unwrap())
}

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

pub fn events_union(ids: &[EventId]) -> EventSet {
    let mut bits = 0i32;
    for id in ids {
        bits |= id.0;
    }
    EventSet(bits)
}

pub fn set_events(py: Python<'_>, tool: &ToolId, set: EventSet) -> PyResult<()> {
    let monitoring = py.import("sys")?.getattr("monitoring")?;
    monitoring.call_method1("set_events", (tool.id, set.0))?;
    Ok(())
}

pub fn free_tool_id(py: Python<'_>, tool: &ToolId) -> PyResult<()> {
    let monitoring = py.import("sys")?.getattr("monitoring")?;
    monitoring.call_method1("free_tool_id", (tool.id,))?;
    Ok(())
}


/// Trait implemented by tracing backends.
///
/// Each method corresponds to an event from `sys.monitoring`. Default
/// implementations allow implementers to only handle the events they care
/// about.
pub trait Tracer: Send {
    /// Return the set of events the tracer wants to receive.
    fn interest(&self, _events: &MonitoringEvents) -> EventSet {
        NO_EVENTS
    }

    /// Called on Python function calls.
    fn on_call(
        &mut self,
        _py: Python<'_>,
        _code: &Bound<'_, PyAny>,
        _offset: i32,
        _callable: &Bound<'_, PyAny>,
        _arg0: Option<&Bound<'_, PyAny>>,
    ) {
    }

    /// Called on line execution.
    fn on_line(&mut self, _py: Python<'_>, _code: &Bound<'_, PyAny>, _lineno: u32) {}
}

struct Global {
    tracer: Box<dyn Tracer>,
    mask: EventSet,
    tool: ToolId,
}

static GLOBAL: Mutex<Option<Global>> = Mutex::new(None);

/// Install a tracer and hook it into Python's `sys.monitoring`.
pub fn install_tracer(py: Python<'_>, tracer: Box<dyn Tracer>) -> PyResult<()> {
    let mut guard = GLOBAL.lock().unwrap();
    if guard.is_some() {
        return Err(PyRuntimeError::new_err("tracer already installed"));
    }

    let tool = acquire_tool_id(py)?;
    let events = monitoring_events(py)?;

    let module = PyModule::new(py, "_codetracer_callbacks")?;

    let mask = tracer.interest(events);

    if mask.contains(&events.CALL) {
        let cb = wrap_pyfunction!(callback_call, &module)?;

        register_callback(py, &tool, &events.CALL, Some(&cb))?;

    }
    if mask.contains(&events.LINE) {
        let cb = wrap_pyfunction!(callback_line, &module)?;
        register_callback(py, &tool, &events.LINE, Some(&cb))?;
    }
    set_events(py, &tool, mask)?;
    

    *guard = Some(Global {
        tracer,
	mask,
        tool,
    });
    Ok(())
}

/// Remove the installed tracer if any.
pub fn uninstall_tracer(py: Python<'_>) -> PyResult<()> {
    let mut guard = GLOBAL.lock().unwrap();
    if let Some(global) = guard.take() {
        let events = monitoring_events(py)?;
        if global.mask.contains(&events.CALL) {
            register_callback(py, &global.tool, &events.CALL, None)?;
        }
        if global.mask.contains(&events.LINE) {
            register_callback(py, &global.tool, &events.LINE, None)?;
        }
        set_events(py, &global.tool, NO_EVENTS)?;
        free_tool_id(py, &global.tool)?;
    }
    Ok(())
}

#[pyfunction]
fn callback_call(
    py: Python<'_>,
    code: Bound<'_, PyAny>,
    offset: i32,
    callable: Bound<'_, PyAny>,
    arg0: Option<Bound<'_, PyAny>>,
) -> PyResult<()> {
    if let Some(global) = GLOBAL.lock().unwrap().as_mut() {
        global.tracer.on_call(py, &code, offset, &callable, arg0.as_ref());
    }
    Ok(())
}

#[pyfunction]
fn callback_line(py: Python<'_>, code: Bound<'_, PyAny>, lineno: u32) -> PyResult<()> {
    if let Some(global) = GLOBAL.lock().unwrap().as_mut() {
        global.tracer.on_line(py, &code, lineno);
    }
    Ok(())
}
