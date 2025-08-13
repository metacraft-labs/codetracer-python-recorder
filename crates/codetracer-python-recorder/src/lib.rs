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
fn codetracer_python_recorder(_py: Python<'_>, m: Bound<'_, PyModule>) -> PyResult<()> {
    let hello_fn = wrap_pyfunction!(hello, &m)?;
    m.add_function(hello_fn)?;
    Ok(())
}
