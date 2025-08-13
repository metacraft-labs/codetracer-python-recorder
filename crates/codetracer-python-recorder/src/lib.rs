use pyo3::prelude::*;

/// codetracer_python_recorder
///
/// Minimal placeholder for the Rust-backed recorder. This exposes a trivial
/// function to verify the module builds and imports successfully.
#[pyfunction]
fn hello() -> PyResult<String> {
    Ok("Hello from codetracer-python-recorder (Rust)".to_string())
}

#[pymodule]
fn codetracer_python_recorder(_py: Python<'_>, m: &PyModule) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(hello, m)?)?;
    Ok(())
}
