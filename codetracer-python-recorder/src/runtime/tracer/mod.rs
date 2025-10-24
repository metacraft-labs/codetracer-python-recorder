//! Collaborators for the runtime tracer lifecycle, IO coordination, filtering, and event handling.
//!
//! Re-exports [`RuntimeTracer`] so downstream callers continue using `crate::runtime::RuntimeTracer`.

pub mod events;
pub mod filtering;
pub mod io;
pub mod lifecycle;

mod runtime_tracer;

pub use runtime_tracer::RuntimeTracer;
