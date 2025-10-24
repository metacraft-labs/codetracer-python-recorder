//! Collaborators for the runtime tracer lifecycle, IO coordination, filtering, and event handling.
//!
//! Re-exports [`RuntimeTracer`] so downstream callers continue using `crate::runtime::RuntimeTracer`
//! without exposing the implementation modules outside the crate.

pub(crate) mod events;
pub(crate) mod filtering;
pub(crate) mod io;
pub(crate) mod lifecycle;

mod runtime_tracer;

pub use runtime_tracer::RuntimeTracer;
