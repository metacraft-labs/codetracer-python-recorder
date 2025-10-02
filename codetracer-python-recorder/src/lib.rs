//! Runtime tracing module backed by PyO3.
//!
//! Tracer implementations must return `CallbackResult` from every callback so they can
//! signal when CPython should disable further monitoring for a location by propagating
//! the `sys.monitoring.DISABLE` sentinel.

pub mod code_object;
mod logging;
mod runtime;
mod session;
pub mod tracer;

pub use crate::code_object::{CodeObjectRegistry, CodeObjectWrapper};
pub use crate::session::{flush_tracing, is_tracing, start_tracing, stop_tracing};
pub use crate::tracer::{
    install_tracer, uninstall_tracer, CallbackOutcome, CallbackResult, EventSet, Tracer,
};

use pyo3::prelude::*;

/// Python module definition.
#[pymodule]
fn codetracer_python_recorder(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Initialize logging on import so users see logs without extra setup.
    // Respect RUST_LOG if present; otherwise default to debug for this crate.
    logging::init_rust_logging_with_default("codetracer_python_recorder=debug");
    m.add_function(wrap_pyfunction!(start_tracing, m)?)?;
    m.add_function(wrap_pyfunction!(stop_tracing, m)?)?;
    m.add_function(wrap_pyfunction!(is_tracing, m)?)?;
    m.add_function(wrap_pyfunction!(flush_tracing, m)?)?;
    Ok(())
}
