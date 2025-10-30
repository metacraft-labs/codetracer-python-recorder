//! Runtime tracing facade wiring sys.monitoring callbacks into dedicated collaborators.
//!
//! The [`tracer`] module hosts lifecycle, IO, filtering, and event pipelines and re-exports
//! [`RuntimeTracer`] so callers can keep importing it from `crate::runtime`.

mod activation;
mod frame_inspector;
pub mod io_capture;
mod line_snapshots;
mod logging;
mod output_paths;
pub mod tracer;
mod value_capture;
pub(crate) mod value_encoder;
pub(crate) mod value_filters;

pub use output_paths::TraceOutputPaths;
pub use tracer::RuntimeTracer;
