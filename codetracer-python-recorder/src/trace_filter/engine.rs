//! Python-aware wrapper around the shared
//! [`codetracer_trace_filter::Classifier`].
//!
//! ## Caching strategy (spec § 6)
//!
//! Per the cross-language trace-filter spec, recorders MUST stash the
//! per-scope classifier decision in the host runtime's native per-scope
//! metadata slot rather than fall back to per-event hash-table lookups.
//!
//! For CPython that slot is `co_extra`. CPython exposes the API
//! `_PyEval_RequestCodeExtraIndex` (allocates one slot index per consumer)
//! plus `_PyCode_GetExtra` / `_PyCode_SetExtra` (read / write a `void*`
//! into the per-code-object slot at the requested index). When the code
//! object is destroyed CPython calls the registered `freefunc` for each
//! occupied slot — this is where the cache entry's refcount gets
//! released, so the cache never leaks memory even though every entry
//! sits behind a raw pointer.
//!
//! The hot path therefore looks like:
//!
//! ```text
//! _PyCode_GetExtra(code, INDEX, &slot);
//! if (slot.exec == Skip) return DISABLE;
//! ```
//!
//! One indirection. No hashes. No DashMap.
//!
//! ## Previous design (replaced by this module)
//!
//! Before TF-M6, the engine kept a `DashMap<CodeId, Arc<ScopeResolution>>`
//! keyed by `code as *const _ as usize`. Every per-event resolution paid
//! one DashMap lookup + one Arc clone. The audit doc flagged this as a
//! spec violation; this module is the replacement.

use crate::code_object::CodeObjectWrapper;
use crate::module_identity::{is_valid_module_name, module_name_from_packages};
use crate::trace_filter::convert_filter_error;
use codetracer_trace_filter::engine::{Classifier, ScopeQuery};
use codetracer_trace_filter::error::FilterError;
use codetracer_trace_filter::model::FilterSummary;
use codetracer_trace_filter::TraceFilterConfig;

// Re-export the pure types so existing call sites that say
// `crate::trace_filter::engine::ExecDecision` continue to compile.
pub use codetracer_trace_filter::engine::{
    CompiledValuePattern, ExecDecision, ScopeResolution, ValueKind, ValuePolicy,
};
pub use codetracer_trace_filter::model::ValueAction;

use once_cell::sync::Lazy;
use pyo3::prelude::*;
use pyo3::PyErr;

// Direct FFI bindings to the CPython `co_extra` slot API.  pyo3-ffi 0.25
// declares the old underscored names (`_PyEval_RequestCodeExtraIndex`,
// `_PyCode_GetExtra`, `_PyCode_SetExtra`) but Python 3.13+ no longer
// exports those as ELF symbols — they survive only as `static inline`
// wrappers in the public header — so linking against `pyo3-ffi`'s
// declarations fails. The replacement names blessed by PEP 689 are
// `PyUnstable_*`; we bind them ourselves to keep the migration
// independent of any future pyo3-ffi release.
//
// References:
//   https://peps.python.org/pep-0689/
//   include/python3.13/cpython/code.h  (declares the PyUnstable_* names)
//   include/python3.13/cpython/ceval.h
extern "C" {
    fn PyUnstable_Eval_RequestCodeExtraIndex(
        func: unsafe extern "C" fn(*mut c_void),
    ) -> pyo3::ffi::Py_ssize_t;
    fn PyUnstable_Code_GetExtra(
        code: *mut pyo3::ffi::PyObject,
        index: pyo3::ffi::Py_ssize_t,
        extra: *mut *mut c_void,
    ) -> std::ffi::c_int;
    fn PyUnstable_Code_SetExtra(
        code: *mut pyo3::ffi::PyObject,
        index: pyo3::ffi::Py_ssize_t,
        extra: *mut c_void,
    ) -> std::ffi::c_int;
}
use recorder_errors::{target, ErrorCode, RecorderResult};
use std::ffi::c_void;
use std::path::Path;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Arc;

/// Global `co_extra` slot index, lazily acquired the first time a
/// `TraceFilterEngine` is constructed.
///
/// CPython hands out exactly one index per call to
/// `_PyEval_RequestCodeExtraIndex`; sharing it across all engine instances
/// on the same interpreter is correct (and necessary — repeated allocation
/// would leak indices that CPython never releases). The atomic uses `-1`
/// as the unset sentinel.
static CODE_EXTRA_INDEX: AtomicIsize = AtomicIsize::new(-1);

/// Initialise the `co_extra` index on first call. Subsequent calls return
/// the cached index value.
///
/// # Safety
///
/// Must be called with the GIL held. `_PyEval_RequestCodeExtraIndex` is a
/// CPython API that mutates interpreter-global state and is not thread-safe
/// without the GIL.
fn ensure_code_extra_index(_py: Python<'_>) -> isize {
    let current = CODE_EXTRA_INDEX.load(Ordering::Acquire);
    if current >= 0 {
        return current;
    }

    // SAFETY: GIL is held by the caller (we accept a `Python<'_>` token).
    let new_index = unsafe { PyUnstable_Eval_RequestCodeExtraIndex(scope_resolution_freefunc) };
    if new_index < 0 {
        // CPython returns -1 on failure. We log and fall back to caching
        // disabled (the engine will then recompute the resolution every
        // call — slow but correct).
        log::error!(
            target: "codetracer_python_recorder::trace_filter",
            "_PyEval_RequestCodeExtraIndex returned {} — trace-filter co_extra caching disabled for this session",
            new_index
        );
        return -1;
    }

    let new_index = new_index as isize;
    // Try to publish our value; the first writer wins.  If a parallel
    // initialiser raced us to a different (also-positive) index, we leak
    // one slot but the recorder still works.
    match CODE_EXTRA_INDEX.compare_exchange(-1, new_index, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => new_index,
        Err(observed) => {
            // Lost the race — keep the previously published index.
            log::debug!(
                target: "codetracer_python_recorder::trace_filter",
                "co_extra index race: acquired {} but {} was already published",
                new_index,
                observed
            );
            observed
        }
    }
}

/// `freefunc` invoked by CPython when a code object holding our slot is
/// destroyed. Drops the `Arc<ScopeResolution>` the slot was pointing at.
///
/// # Safety
///
/// CPython invariants: this is only called with a pointer previously
/// stored via `_PyCode_SetExtra` for our index, or NULL. It is called at
/// most once per code-object destruction.
unsafe extern "C" fn scope_resolution_freefunc(payload: *mut c_void) {
    if payload.is_null() {
        return;
    }
    // Recover the Arc that was leaked when we stored it in the slot, then
    // drop it. This balances the refcount taken at insertion time.
    drop(Arc::from_raw(payload as *const ScopeResolution));
}

/// Python-aware filter engine wrapping the shared crate's [`Classifier`].
///
/// Maintains a singleton-per-recorder `co_extra` slot index so that every
/// `PyCode` object can carry its own classifier decision without ever
/// hitting a hash map on the trace-emission hot path.
pub struct TraceFilterEngine {
    classifier: Arc<Classifier>,
    /// Slot index returned by `_PyEval_RequestCodeExtraIndex`. A negative
    /// value disables caching (logged as a warning at construction time).
    code_extra_index: isize,
}

impl TraceFilterEngine {
    /// Construct the engine from a fully resolved configuration.
    pub fn new(config: TraceFilterConfig) -> Self {
        let classifier = Classifier::new(config);
        Python::with_gil(|py| Self {
            classifier: Arc::new(classifier),
            code_extra_index: ensure_code_extra_index(py),
        })
    }

    /// Resolve the scope decision for `code`, reusing the cached result
    /// stashed in `co_extra` when available.
    ///
    /// On a hit this is one CPython call (`_PyCode_GetExtra`) plus one
    /// `Arc::clone`. On a miss the classifier runs once, the result is
    /// stored, and future calls hit the cache.
    pub fn resolve(
        &self,
        py: Python<'_>,
        code: &CodeObjectWrapper,
        module_hint: Option<&str>,
    ) -> RecorderResult<Arc<ScopeResolution>> {
        if self.code_extra_index >= 0 {
            // SAFETY: GIL is held (we have a `Python` token). The slot
            // index was returned by `_PyEval_RequestCodeExtraIndex` and is
            // valid for the lifetime of the interpreter.
            unsafe {
                let mut slot: *mut c_void = std::ptr::null_mut();
                let code_ptr = code.as_bound(py).as_ptr();
                let rc = PyUnstable_Code_GetExtra(code_ptr, self.code_extra_index, &mut slot);
                if rc == 0 && !slot.is_null() {
                    // Slot occupied: bump the refcount on the cached Arc
                    // without taking ownership of the leaked pointer.
                    let raw = slot as *const ScopeResolution;
                    Arc::increment_strong_count(raw);
                    return Ok(Arc::from_raw(raw));
                }
                // If rc != 0 CPython has already cleared the error
                // indicator; treat it like an empty slot.
            }
        }

        // Cache miss (or caching disabled): classify and (try to) store.
        let resolution = Arc::new(self.classify(py, code, module_hint)?);

        if self.code_extra_index >= 0 {
            // SAFETY: leak one strong refcount on the Arc by converting
            // it to a raw pointer; the matching `Arc::from_raw` happens in
            // `scope_resolution_freefunc` when CPython destroys the code
            // object.  This is how `_PyCode_SetExtra` slots work in
            // practice.
            unsafe {
                let raw = Arc::into_raw(Arc::clone(&resolution)) as *mut c_void;
                let code_ptr = code.as_bound(py).as_ptr();
                let rc = PyUnstable_Code_SetExtra(code_ptr, self.code_extra_index, raw);
                if rc != 0 {
                    // CPython refused our slot write (rare — typically OOM
                    // or a permission issue).  Reclaim the leaked refcount
                    // and fall through; the per-event cost regresses to a
                    // miss-every-call rather than leaking memory.
                    let _ = Arc::from_raw(raw as *const ScopeResolution);
                    log::warn!(
                        target: "codetracer_python_recorder::trace_filter",
                        "_PyCode_SetExtra returned {} — trace-filter cache write failed",
                        rc
                    );
                }
            }
        }

        Ok(resolution)
    }

    fn classify(
        &self,
        py: Python<'_>,
        code: &CodeObjectWrapper,
        module_hint: Option<&str>,
    ) -> RecorderResult<ScopeResolution> {
        // Resolve the filename + qualname from the code object once;
        // these strings are then borrowed into the ScopeQuery without
        // further allocation.
        let filename = code
            .filename(py)
            .map_err(|err| py_attr_error("co_filename", err))?;
        let qualname = code
            .qualname(py)
            .map_err(|err| py_attr_error("co_qualname", err))?;

        // Build the initial ScopeQuery and run a first classification pass.
        // The classifier itself does best-effort module-name derivation from
        // the filename. If it still couldn't produce a module name we try
        // the `__init__.py`-walking fallback (Python-specific) before
        // running a second classification pass.
        let mut query = ScopeQuery::new(filename).with_qualname(qualname);
        if let Some(hint) = module_hint {
            query = query.with_module_hint(hint);
        }
        let resolution = self.classifier.classify(&query);

        // If the classifier saw no usable module name, try the package
        // discovery fallback from the recorder's module_identity helpers.
        if resolution.module_name().is_none() {
            if let Some(absolute) = resolution.absolute_path() {
                if let Some(derived) = module_name_from_packages(Path::new(absolute)) {
                    if is_valid_module_name(&derived) {
                        let query = ScopeQuery::new(filename)
                            .with_qualname(qualname)
                            .with_module_hint(&derived);
                        return Ok(self.classifier.classify(&query));
                    }
                }
            }
        }

        Ok(resolution)
    }

    /// Return a summary of the filters that produced this engine.
    pub fn summary(&self) -> FilterSummary {
        self.classifier.summary()
    }

    /// Borrow the underlying compiled classifier.  Useful for unit tests
    /// that bypass the Python code-object layer.
    pub fn classifier(&self) -> &Classifier {
        &self.classifier
    }
}

/// Adapter so callers in the recorder that historically reached for
/// `crate::trace_filter::engine::TraceFilterEngine` continue to receive
/// the `RecorderResult` flavour of errors.
#[allow(dead_code)]
pub(crate) fn convert_error(err: FilterError) -> recorder_errors::RecorderError {
    convert_filter_error(err)
}

fn py_attr_error(attr: &str, err: PyErr) -> recorder_errors::RecorderError {
    target!(
        ErrorCode::FrameIntrospectionFailed,
        "failed to read {} from code object: {}",
        attr,
        err
    )
}

/// Lazy access to the global code-extra index for tests that want to
/// observe its current state.
#[cfg(test)]
pub(crate) fn current_code_extra_index() -> Option<isize> {
    let value = CODE_EXTRA_INDEX.load(Ordering::Acquire);
    if value < 0 {
        None
    } else {
        Some(value)
    }
}

// The Lazy import is here only to discourage future contributors from
// reintroducing a HashMap cache "for tests"; the `co_extra`-based path is
// the only blessed one.
#[allow(dead_code)]
static _DO_NOT_USE_HASH_CACHE: Lazy<()> = Lazy::new(|| ());

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace_filter::config::TraceFilterConfig;
    use pyo3::types::{PyAny, PyCode, PyList, PyModule};
    use recorder_errors::ErrorCode;
    use std::ffi::CString;
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    fn filter_with_pkg_rule(body: &str) -> RecorderResult<(TraceFilterConfig, String)> {
        let temp = tempdir().expect("temp dir");
        let project_root = temp.path();
        let codetracer_dir = project_root.join(".codetracer");
        fs::create_dir(&codetracer_dir).expect("create .codetracer dir");

        let filter_path = codetracer_dir.join("filters.toml");
        write_filter(&filter_path, body);

        let config = TraceFilterConfig::from_paths(&[filter_path]).map_err(convert_error)?;

        let file_path = project_root.join("app").join("foo.py");
        fs::create_dir_all(file_path.parent().expect("parent")).expect("create parent dirs");
        fs::File::create(&file_path).expect("create file");
        // Keep the temp directory alive for the duration of the test.
        std::mem::forget(temp);

        Ok((config, file_path.to_string_lossy().to_string()))
    }

    fn write_filter(path: &Path, body: &str) {
        let mut file = fs::File::create(path).expect("create filter file");
        writeln!(
            file,
            r#"
            [meta]
            name = "test"
            version = 1

            {}
            "#,
            body.trim()
        )
        .expect("write filter content");
    }

    fn load_module<'py>(
        py: Python<'py>,
        module_name: &str,
        file_path: &str,
        source: &str,
    ) -> RecorderResult<Bound<'py, PyModule>> {
        let code_c = CString::new(source).expect("source without NUL");
        let file_c = CString::new(file_path).expect("path without NUL");
        let module_c = CString::new(module_name).expect("module without NUL");

        let module = PyModule::from_code(
            py,
            code_c.as_c_str(),
            file_c.as_c_str(),
            module_c.as_c_str(),
        )
        .map_err(|err| {
            target!(
                ErrorCode::FrameIntrospectionFailed,
                "failed to load module for engine test: {}",
                err
            )
        })?;
        Ok(module)
    }

    fn get_code<'py>(
        module: &Bound<'py, PyModule>,
        func_name: &str,
    ) -> RecorderResult<Bound<'py, PyCode>> {
        let func: Bound<'py, PyAny> = module
            .getattr(func_name)
            .map_err(|err| py_attr_error("function", err))?;
        let code_obj = func
            .getattr("__code__")
            .map_err(|err| py_attr_error("__code__", err))?
            .downcast_into::<PyCode>()
            .map_err(|err| py_attr_error("__code__", err.into()))?;
        Ok(code_obj)
    }

    #[test]
    fn caches_resolution_via_co_extra() -> RecorderResult<()> {
        let (config, file_path) = filter_with_pkg_rule(
            r#"
            [scope]
            default_exec = "skip"
            default_value_action = "redact"

            [[scope.rules]]
            selector = "pkg:app.foo"
            exec = "trace"
            value_default = "allow"

            [[scope.rules.value_patterns]]
            selector = "local:literal:user"
            action = "allow"

            [[scope.rules.value_patterns]]
            selector = "arg:password"
            action = "redact"
            "#,
        )?;

        Python::with_gil(|py| -> RecorderResult<()> {
            let module = load_module(
                py,
                "app.foo",
                &file_path,
                "def foo(user, password):\n    return user\n",
            )?;
            let code_obj = get_code(&module, "foo")?;
            let wrapper = CodeObjectWrapper::new(py, &code_obj);
            let engine = TraceFilterEngine::new(config.clone());

            let first = engine.resolve(py, &wrapper, None)?;
            assert_eq!(first.exec(), ExecDecision::Trace);
            assert_eq!(first.module_name(), Some("app.foo"));
            assert_eq!(first.relative_path(), Some("app/foo.py"));

            let policy = first.value_policy();
            assert_eq!(policy.default_action(), ValueAction::Allow);
            assert_eq!(policy.decide(ValueKind::Local, "user"), ValueAction::Allow);
            assert_eq!(
                policy.decide(ValueKind::Arg, "password"),
                ValueAction::Redact
            );

            // Second resolve should observe the cached value (co_extra hit).
            let second = engine.resolve(py, &wrapper, None)?;
            assert_eq!(second.exec(), ExecDecision::Trace);

            // The slot is reachable through CPython directly.
            let index = current_code_extra_index().expect("co_extra index allocated");
            let mut slot: *mut std::ffi::c_void = std::ptr::null_mut();
            let rc = unsafe { PyUnstable_Code_GetExtra(code_obj.as_ptr(), index, &mut slot) };
            assert_eq!(rc, 0, "_PyCode_GetExtra failed");
            assert!(!slot.is_null(), "co_extra slot must be populated");
            Ok(())
        })
    }

    #[test]
    fn object_rule_overrides_package_rule() -> RecorderResult<()> {
        let (config, file_path) = filter_with_pkg_rule(
            r#"
            [scope]
            default_exec = "trace"
            default_value_action = "allow"

            [[scope.rules]]
            selector = "pkg:app.foo"
            exec = "skip"

            [[scope.rules]]
            selector = "obj:app.foo.bar"
            exec = "trace"
            value_default = "redact"
            "#,
        )?;

        Python::with_gil(|py| -> RecorderResult<()> {
            let module = load_module(
                py,
                "app.foo",
                &file_path,
                "def bar():\n    secret = 1\n    return secret\n",
            )?;
            let code_obj = get_code(&module, "bar")?;
            let wrapper = CodeObjectWrapper::new(py, &code_obj);

            let engine = TraceFilterEngine::new(config);
            let resolution = engine.resolve(py, &wrapper, None)?;

            assert_eq!(resolution.exec(), ExecDecision::Trace);
            assert_eq!(resolution.matched_rule_index(), Some(1));
            assert_eq!(
                resolution.value_policy().default_action(),
                ValueAction::Redact
            );
            Ok(())
        })
    }

    #[test]
    fn inline_pkg_rule_uses_sys_modules_fallback() -> RecorderResult<()> {
        let inline = r#"
            [meta]
            name = "inline"
            version = 1

            [scope]
            default_exec = "trace"
            default_value_action = "allow"

            [[scope.rules]]
            selector = "pkg:literal:app.foo"
            exec = "skip"
        "#;
        let config = TraceFilterConfig::from_inline_and_paths(&[("inline", inline)], &[])
            .map_err(convert_error)?;

        Python::with_gil(|py| -> RecorderResult<()> {
            let project = tempdir().expect("project");
            let project_root = project.path();
            let app_dir = project_root.join("app");
            fs::create_dir_all(&app_dir).expect("create app dir");
            let file_path = app_dir.join("foo.py");
            fs::write(
                &file_path,
                "def foo():\n    secret = 42\n    return secret\n",
            )
            .expect("write module");

            fs::write(app_dir.join("__init__.py"), "\n").expect("write __init__");
            let sys = py.import("sys").expect("import sys");
            let sys_path_any = sys.getattr("path").expect("sys.path");
            let sys_path: Bound<'_, PyList> =
                sys_path_any.downcast_into::<PyList>().expect("path list");
            sys_path
                .insert(0, project_root.to_string_lossy().to_string())
                .expect("insert project root");

            let module = py.import("app.foo").expect("import app.foo");
            let func: Bound<'_, PyAny> = module.getattr("foo").expect("get foo");
            let code_obj = func
                .getattr("__code__")
                .expect("__code__")
                .downcast_into::<PyCode>()
                .expect("PyCode");
            let wrapper = CodeObjectWrapper::new(py, &code_obj);

            let engine = TraceFilterEngine::new(config);
            let resolution = engine.resolve(py, &wrapper, None)?;
            assert_eq!(resolution.module_name(), Some("app.foo"));
            assert_eq!(resolution.exec(), ExecDecision::Skip);

            sys_path.del_item(0).expect("restore sys.path");
            // Keep the temp dir alive across the rest of the test.
            std::mem::forget(project);
            // Silence the "imported PyList unused" warning by referencing
            // `PathBuf` (used by the convert_error utility import path).
            let _ = PathBuf::new();
            Ok(())
        })
    }
}
