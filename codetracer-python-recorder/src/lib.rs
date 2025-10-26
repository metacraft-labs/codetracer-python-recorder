//! Runtime tracing module backed by PyO3.
//!
//! Tracer implementations must return `CallbackResult` from every callback so they can
//! signal when CPython should disable further monitoring for a location by propagating
//! the `sys.monitoring.DISABLE` sentinel.

pub mod code_object;
mod errors;
mod ffi;
mod logging;
mod module_identity;
pub mod monitoring;
mod policy;
mod runtime;
mod session;
pub mod trace_filter;

pub use crate::code_object::{CodeObjectRegistry, CodeObjectWrapper};
pub use crate::monitoring as tracer;
pub use crate::monitoring::{
    flush_installed_tracer, install_tracer, uninstall_tracer, CallbackOutcome, CallbackResult,
    EventSet, Tracer,
};
pub use crate::session::{flush_tracing, is_tracing, start_tracing, stop_tracing};

use pyo3::prelude::*;

/// Python module definition.
#[pymodule]
fn codetracer_python_recorder(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Initialize logging on import so users see logs without extra setup.
    // Respect RUST_LOG if present; otherwise keep our crate quiet unless warnings/errors occur.
    logging::init_rust_logging_with_default("codetracer_python_recorder=warn");
    ffi::register_exceptions(m)?;
    m.add_function(wrap_pyfunction!(start_tracing, m)?)?;
    m.add_function(wrap_pyfunction!(stop_tracing, m)?)?;
    m.add_function(wrap_pyfunction!(is_tracing, m)?)?;
    m.add_function(wrap_pyfunction!(flush_tracing, m)?)?;
    m.add_function(wrap_pyfunction!(policy::configure_policy_py, m)?)?;
    m.add_function(wrap_pyfunction!(policy::py_configure_policy_from_env, m)?)?;
    m.add_function(wrap_pyfunction!(policy::py_policy_snapshot, m)?)?;
    Ok(())
}
