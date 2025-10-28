//! Activation gating for the runtime tracer.

use std::path::{Path, PathBuf};

use pyo3::Python;

use crate::code_object::CodeObjectWrapper;

/// Tracks activation gating for the runtime tracer. When configured with an
/// activation path, tracing remains paused until code from that file starts
/// executing. Once the activation window completes, tracing is disabled for the
/// remainder of the session.
#[derive(Debug)]
pub struct ActivationController {
    activation_path: Option<PathBuf>,
    activation_code_id: Option<usize>,
    activation_done: bool,
    started: bool,
    suspended: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivationExitKind {
    Suspended,
    Completed,
}

impl ActivationController {
    pub fn new(activation_path: Option<&Path>) -> Self {
        let activation_path = activation_path
            .map(|p| std::path::absolute(p).expect("activation_path should resolve"));
        let started = activation_path.is_none();
        Self {
            activation_path,
            activation_code_id: None,
            activation_done: false,
            started,
            suspended: false,
        }
    }

    pub fn is_active(&self) -> bool {
        self.started
    }

    /// Ensure activation state reflects the current event and report whether
    /// tracing should continue processing it.
    pub fn should_process_event(&mut self, py: Python<'_>, code: &CodeObjectWrapper) -> bool {
        self.resume_if_needed(code);
        self.ensure_started(py, code);
        self.is_active()
    }

    /// Return the canonical start path for writer initialisation.
    pub fn start_path<'a>(&'a self, fallback: &'a Path) -> &'a Path {
        self.activation_path.as_deref().unwrap_or(fallback)
    }

    /// Attempt to transition into the active state. When the code object
    /// corresponds to the activation path, tracing becomes active and remembers
    /// the triggering code id so it can stop on return.
    pub fn ensure_started(&mut self, py: Python<'_>, code: &CodeObjectWrapper) {
        if self.started || self.activation_done {
            return;
        }
        if let Some(activation) = &self.activation_path {
            if let Ok(filename) = code.filename(py) {
                let file = Path::new(filename);
                // `CodeObjectWrapper::filename` is expected to return an absolute
                // path. If this assumption turns out to be wrong we will revisit
                // the comparison logic. Canonicalisation is deliberately avoided
                // here to limit syscalls on hot paths.
                if file == activation {
                    self.started = true;
                    self.activation_code_id = Some(code.id());
                    log::debug!(
                        "[RuntimeTracer] activated on enter: {}",
                        activation.display()
                    );
                }
            }
        }
    }

    /// Handle activation exits, marking suspension or completion as appropriate.
    /// Returns `true` when tracing was deactivated by this call.
    pub fn handle_exit(&mut self, code_id: usize, exit: ActivationExitKind) -> bool {
        if self.activation_code_id != Some(code_id) {
            return false;
        }
        match exit {
            ActivationExitKind::Suspended => {
                self.suspended = true;
                false
            }
            ActivationExitKind::Completed => {
                self.started = false;
                self.activation_done = true;
                self.suspended = false;
                true
            }
        }
    }

    fn resume_if_needed(&mut self, code: &CodeObjectWrapper) {
        if self.started
            && self.suspended
            && self.activation_code_id == Some(code.id())
        {
            self.suspended = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::types::{PyAnyMethods, PyCode, PyModule};
    use pyo3::{Bound, Python};
    use std::ffi::CString;

    fn build_code(py: Python<'_>, name: &str, filename: &str) -> CodeObjectWrapper {
        let module_src = format!("def {name}():\n    return 42\n");
        let c_src = CString::new(module_src).expect("source");
        let c_filename = CString::new(filename).expect("filename");
        let c_module = CString::new("m").expect("module");
        let module = PyModule::from_code(
            py,
            c_src.as_c_str(),
            c_filename.as_c_str(),
            c_module.as_c_str(),
        )
        .expect("compile module");
        let func = module.getattr(name).expect("fetch function");
        let code: Bound<'_, PyCode> = func
            .getattr("__code__")
            .expect("__code__")
            .downcast_into()
            .expect("code");
        CodeObjectWrapper::new(py, &code)
    }

    #[test]
    fn starts_active_when_no_activation_path() {
        let controller = ActivationController::new(None);
        assert!(controller.is_active());
    }

    #[test]
    fn remains_inactive_until_activation_code_runs() {
        Python::with_gil(|py| {
            let code = build_code(py, "target", "/tmp/target.py");
            let mut controller = ActivationController::new(Some(Path::new("/tmp/target.py")));
            assert!(!controller.is_active());
            assert!(controller.should_process_event(py, &code));
            assert!(controller.is_active());
        });
    }

    #[test]
    fn ignores_non_matching_code_objects() {
        Python::with_gil(|py| {
            let code = build_code(py, "other", "/tmp/other.py");
            let mut controller = ActivationController::new(Some(Path::new("/tmp/target.py")));
            assert!(!controller.should_process_event(py, &code));
            assert!(!controller.is_active());
        });
    }

    #[test]
    fn deactivates_after_activation_return() {
        Python::with_gil(|py| {
            let code = build_code(py, "target", "/tmp/target.py");
            let mut controller = ActivationController::new(Some(Path::new("/tmp/target.py")));
            assert!(controller.should_process_event(py, &code));
            assert!(controller.is_active());
            assert!(controller.handle_exit(code.id(), ActivationExitKind::Completed));
            assert!(!controller.is_active());
            assert!(!controller.should_process_event(py, &code));
        });
    }

    #[test]
    fn suspension_keeps_tracing_active() {
        Python::with_gil(|py| {
            let code = build_code(py, "target", "/tmp/target.py");
            let mut controller = ActivationController::new(Some(Path::new("/tmp/target.py")));
            assert!(controller.should_process_event(py, &code));
            assert!(controller.is_active());
            assert!(!controller.handle_exit(code.id(), ActivationExitKind::Suspended));
            // While suspended, subsequent events should keep tracing alive and clear suspension
            assert!(controller.should_process_event(py, &code));
            assert!(controller.is_active());
            assert!(controller.handle_exit(code.id(), ActivationExitKind::Completed));
            assert!(!controller.is_active());
        });
    }

    #[test]
    fn start_path_prefers_activation_path() {
        let controller = ActivationController::new(Some(Path::new("/tmp/target.py")));
        let fallback = Path::new("/tmp/fallback.py");
        assert_eq!(controller.start_path(fallback), Path::new("/tmp/target.py"));
    }
}

impl ActivationController {
    #[allow(dead_code)]
    pub fn handle_return(&mut self, code_id: usize) -> bool {
        self.handle_exit(code_id, ActivationExitKind::Completed)
    }
}
