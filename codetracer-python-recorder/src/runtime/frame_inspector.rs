//! Safe helpers around CPython frame inspection for tracing callbacks.

use std::ptr;

use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict, PyMapping};
use pyo3::{ffi, Py, PyErr};
use recorder_errors::{enverr, ErrorCode};

use crate::code_object::CodeObjectWrapper;
use crate::ffi::map_recorder_error;

extern "C" {
    fn PyFrame_GetLocals(frame: *mut ffi::PyFrameObject) -> *mut ffi::PyObject;
    fn PyFrame_GetGlobals(frame: *mut ffi::PyFrameObject) -> *mut ffi::PyObject;
}

/// Snapshot of the current frame including materialised locals and globals.
#[derive(Debug)]
pub struct FrameSnapshot<'py> {
    frame_ptr: *mut ffi::PyFrameObject,
    locals: Bound<'py, PyDict>,
    globals: Option<Bound<'py, PyDict>>,
    locals_is_globals: bool,
}

impl<'py> FrameSnapshot<'py> {
    /// Borrow the snapshot of locals for iteration.
    pub fn locals(&self) -> &Bound<'py, PyDict> {
        &self.locals
    }

    /// Borrow the snapshot of globals when distinct from locals.
    pub fn globals(&self) -> Option<&Bound<'py, PyDict>> {
        self.globals.as_ref()
    }

    /// Return true when the original frame referenced the same dict for
    /// locals and globals.
    pub fn locals_is_globals(&self) -> bool {
        self.locals_is_globals
    }

    /// Expose the raw frame pointer for correlation purposes.
    pub fn frame_ptr(&self) -> *mut ffi::PyFrameObject {
        self.frame_ptr
    }
}

impl<'py> Drop for FrameSnapshot<'py> {
    fn drop(&mut self) {
        if !self.frame_ptr.is_null() {
            unsafe {
                ffi::Py_DECREF(self.frame_ptr.cast());
            }
            self.frame_ptr = ptr::null_mut();
        }
    }
}

/// Capture the frame for *code* and materialise its locals/globals mappings.
///
/// Returns a RAII snapshot ensuring reference counts are decremented when the
/// snapshot leaves scope.
pub fn capture_frame<'py>(
    py: Python<'py>,
    code: &CodeObjectWrapper,
) -> PyResult<FrameSnapshot<'py>> {
    let mut frame_ptr = unsafe { ffi::PyEval_GetFrame() };
    if frame_ptr.is_null() {
        return Err(map_recorder_error(enverr!(
            ErrorCode::FrameIntrospectionFailed,
            "PyEval_GetFrame returned null frame"
        )));
    }

    unsafe {
        ffi::Py_XINCREF(frame_ptr.cast());
    }

    let target_code_ptr = code.as_bound(py).as_ptr();

    loop {
        if frame_ptr.is_null() {
            break;
        }
        let frame_code_ptr = unsafe { ffi::PyFrame_GetCode(frame_ptr) };
        if frame_code_ptr.is_null() {
            unsafe {
                ffi::Py_DECREF(frame_ptr.cast());
            }
            return Err(map_recorder_error(enverr!(
                ErrorCode::FrameIntrospectionFailed,
                "PyFrame_GetCode returned null"
            )));
        }
        let frame_code: Py<PyAny> = unsafe { Py::from_owned_ptr(py, frame_code_ptr.cast()) };
        if frame_code.as_ptr() == target_code_ptr {
            break;
        }
        let back = unsafe { ffi::PyFrame_GetBack(frame_ptr) };
        unsafe {
            ffi::Py_DECREF(frame_ptr.cast());
        }
        frame_ptr = back;
    }

    if frame_ptr.is_null() {
        return Err(map_recorder_error(enverr!(
            ErrorCode::FrameIntrospectionFailed,
            "Failed to locate frame for code object"
        )));
    }

    unsafe {
        if ffi::PyFrame_FastToLocalsWithError(frame_ptr) < 0 {
            ffi::Py_DECREF(frame_ptr.cast());
            let err = PyErr::fetch(py);
            return Err(err);
        }
    }

    let locals_raw = unsafe { PyFrame_GetLocals(frame_ptr) };
    if locals_raw.is_null() {
        unsafe {
            ffi::Py_DECREF(frame_ptr.cast());
        }
        return Err(map_recorder_error(enverr!(
            ErrorCode::FrameIntrospectionFailed,
            "PyFrame_GetLocals returned null"
        )));
    }
    let locals_any = unsafe { Bound::<PyAny>::from_owned_ptr(py, locals_raw.cast()) };
    let locals_mapping = locals_any.downcast::<PyMapping>().map_err(|_| {
        map_recorder_error(enverr!(
            ErrorCode::FrameIntrospectionFailed,
            "Frame locals was not a mapping"
        ))
    })?;

    let globals_raw = unsafe { PyFrame_GetGlobals(frame_ptr) };
    if globals_raw.is_null() {
        unsafe {
            ffi::Py_DECREF(frame_ptr.cast());
        }
        return Err(map_recorder_error(enverr!(
            ErrorCode::GlobalsIntrospectionFailed,
            "PyFrame_GetGlobals returned null"
        )));
    }
    let globals_any = unsafe { Bound::<PyAny>::from_owned_ptr(py, globals_raw.cast()) };
    let globals_mapping = globals_any.downcast::<PyMapping>().map_err(|_| {
        map_recorder_error(enverr!(
            ErrorCode::GlobalsIntrospectionFailed,
            "Frame globals was not a mapping"
        ))
    })?;

    let locals_is_globals = locals_raw == globals_raw;

    let locals_dict = PyDict::new(py);
    locals_dict.update(&locals_mapping).map_err(|err| {
        map_recorder_error(
            enverr!(
                ErrorCode::FrameIntrospectionFailed,
                "Failed to materialize locals dict"
            )
            .with_context("details", err.to_string()),
        )
    })?;

    let globals_dict = if locals_is_globals {
        None
    } else {
        let dict = PyDict::new(py);
        dict.update(&globals_mapping).map_err(|err| {
            map_recorder_error(
                enverr!(
                    ErrorCode::GlobalsIntrospectionFailed,
                    "Failed to materialize globals dict"
                )
                .with_context("details", err.to_string()),
            )
        })?;
        Some(dict)
    };

    Ok(FrameSnapshot {
        frame_ptr,
        locals: locals_dict,
        globals: globals_dict,
        locals_is_globals,
    })
}
