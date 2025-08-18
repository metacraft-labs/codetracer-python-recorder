use std::sync::Mutex;

use bitflags::bitflags;
use pyo3::{
    exceptions::{PyRuntimeError, PyTypeError},
    ffi,
    prelude::*,
    types::{PyModule, PyTuple},
};

bitflags! {
    /// Bitmask of monitoring events a tracer can subscribe to.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct EventMask: u32 {
        /// Python function calls.
        const CALL = 1 << 0;
        /// Line execution events.
        const LINE = 1 << 1;
    }
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Call,
    Line,
}

/// Trait implemented by tracing backends.
///
/// Each method corresponds to an event from `sys.monitoring`. Default
/// implementations allow implementers to only handle the events they care
/// about.
pub trait Tracer: Send {
    /// Return the set of events the tracer wants to receive.
    fn interest(&self) -> EventMask {
        EventMask::empty()
    }

    /// Called on Python function calls.
    fn on_call(&mut self, _py: Python<'_>, _frame: *mut ffi::PyFrameObject) {}

    /// Called on line execution.
    fn on_line(&mut self, _py: Python<'_>, _frame: *mut ffi::PyFrameObject, _lineno: u32) {}
}

/// Dispatcher routing events based on the tracer's interest mask.
pub struct Dispatcher {
    tracer: Box<dyn Tracer>,
    mask: EventMask,
}

impl Dispatcher {
    pub fn new(tracer: Box<dyn Tracer>) -> Self {
        let mask = tracer.interest();
        Self { tracer, mask }
    }

    pub fn dispatch_call(&mut self, py: Python<'_>, frame: *mut ffi::PyFrameObject) {
        if self.mask.contains(EventMask::CALL) {
            self.tracer.on_call(py, frame);
        }
    }

    pub fn dispatch_line(&mut self, py: Python<'_>, frame: *mut ffi::PyFrameObject, lineno: u32) {
        if self.mask.contains(EventMask::LINE) {
            self.tracer.on_line(py, frame, lineno);
        }
    }
}

struct Global {
    dispatcher: Dispatcher,
    tool_id: u8,
    callbacks: Vec<Py<PyAny>>,
}

static GLOBAL: Mutex<Option<Global>> = Mutex::new(None);

/// Install a tracer and hook it into Python's `sys.monitoring`.
pub fn install_tracer(py: Python<'_>, tracer: Box<dyn Tracer>) -> PyResult<()> {
    let mask = tracer.interest();
    let mut guard = GLOBAL.lock().unwrap();
    if guard.is_some() {
        return Err(PyRuntimeError::new_err("tracer already installed"));
    }
    let monitoring = py.import("sys")?.getattr("monitoring")?;
    // `use_tool_id` changed its signature between Python versions.
    // Try calling it with the newer single-argument form first and fall back to
    // the older two-argument variant if that fails with a `TypeError`.
    const FALLBACK_ID: u8 = 5;
    let tool_id: u8 = match monitoring.call_method1("use_tool_id", ("codetracer",)) {
        Ok(obj) => obj.extract()?,
        Err(err) => {
            if err.is_instance_of::<PyTypeError>(py) {
                monitoring.call_method1("use_tool_id", (FALLBACK_ID, "codetracer"))?;
                FALLBACK_ID
            } else {
                return Err(err);
            }
        }
    };
    let events = monitoring.getattr("events")?;
    let module = PyModule::new(py, "_codetracer_callbacks")?;

    let mut callbacks = Vec::new();
    if mask.contains(EventMask::CALL) {
        module.add_function(wrap_pyfunction!(callback_call, &module)?)?;
        let cb = module.getattr("callback_call")?;
        let ev = events.getattr("CALL")?;
        monitoring.call_method("register_callback", (tool_id, ev, &cb), None)?;
        callbacks.push(cb.unbind());
    }
    if mask.contains(EventMask::LINE) {
        module.add_function(wrap_pyfunction!(callback_line, &module)?)?;
        let cb = module.getattr("callback_line")?;
        let ev = events.getattr("LINE")?;
        monitoring.call_method("register_callback", (tool_id, ev, &cb), None)?;
        callbacks.push(cb.unbind());
    }
    if let Err(err) =
        monitoring.call_method("set_events", (tool_id, mask.bits(), mask.bits()), None)
    {
        if err.is_instance_of::<PyTypeError>(py) {
            monitoring.call_method1("set_events", (tool_id, mask.bits()))?;
        } else {
            return Err(err);
        }
    }

    *guard = Some(Global {
        dispatcher: Dispatcher::new(tracer),
        tool_id,
        callbacks,
    });
    Ok(())
}

/// Remove the installed tracer if any.
pub fn uninstall_tracer(py: Python<'_>) -> PyResult<()> {
    let mut guard = GLOBAL.lock().unwrap();
    if let Some(global) = guard.take() {
        let monitoring = py.import("sys")?.getattr("monitoring")?;
        let events = monitoring.getattr("events")?;
        if global.dispatcher.mask.contains(EventMask::CALL) {
            let ev = events.getattr("CALL")?;
            monitoring.call_method("register_callback", (global.tool_id, ev, py.None()), None)?;
        }
        if global.dispatcher.mask.contains(EventMask::LINE) {
            let ev = events.getattr("LINE")?;
            monitoring.call_method("register_callback", (global.tool_id, ev, py.None()), None)?;
        }
        if let Err(err) = monitoring.call_method(
            "set_events",
            (global.tool_id, 0u32, global.dispatcher.mask.bits()),
            None,
        ) {
            if err.is_instance_of::<PyTypeError>(py) {
                monitoring.call_method1("set_events", (global.tool_id, 0u32))?;
            } else {
                return Err(err);
            }
        }
        monitoring.call_method1("free_tool_id", (global.tool_id,))?;
    }
    Ok(())
}

#[pyfunction]
fn callback_call(py: Python<'_>, args: &Bound<'_, PyTuple>) -> PyResult<()> {
    let frame = args.get_item(0)?.as_ptr() as *mut ffi::PyFrameObject;
    if let Some(global) = GLOBAL.lock().unwrap().as_mut() {
        global.dispatcher.dispatch_call(py, frame);
    }
    Ok(())
}

#[pyfunction]
fn callback_line(py: Python<'_>, args: &Bound<'_, PyTuple>) -> PyResult<()> {
    let frame = args.get_item(0)?.as_ptr() as *mut ffi::PyFrameObject;
    let lineno: u32 = args.get_item(1)?.extract()?;
    if let Some(global) = GLOBAL.lock().unwrap().as_mut() {
        global.dispatcher.dispatch_line(py, frame, lineno);
    }
    Ok(())
}
