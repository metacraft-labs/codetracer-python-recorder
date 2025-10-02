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
        }
    }

    pub fn is_active(&self) -> bool {
        self.started
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

    /// Handle return events and turn off tracing when the activation function
    /// exits. Returns `true` when tracing was deactivated by this call.
    pub fn handle_return(&mut self, code_id: usize) -> bool {
        if self.activation_code_id == Some(code_id) {
            self.started = false;
            self.activation_done = true;
            return true;
        }
        false
    }
}
