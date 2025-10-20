//! Runtime tracer facade translating sys.monitoring callbacks into `runtime_tracing` records.

mod activation;
mod frame_inspector;
pub mod io_capture;
mod line_snapshots;
mod logging;
mod output_paths;
pub mod tracer;
mod value_capture;
mod value_encoder;

#[allow(unused_imports)]
pub use line_snapshots::{FrameId, LineSnapshotStore};
pub use output_paths::TraceOutputPaths;
pub use tracer::RuntimeTracer;
