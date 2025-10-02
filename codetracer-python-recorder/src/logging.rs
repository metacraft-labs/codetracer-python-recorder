//! Process-wide logging helpers shared by the PyO3 entry points and tests.

use std::sync::Once;

/// Initialise the process-wide Rust logger with a default filter.
///
/// The logger is only set up once per process. Callers can override the filter
/// by setting the `RUST_LOG` environment variable before the first invocation.
pub fn init_rust_logging_with_default(default_filter: &str) {
    static INIT_LOGGER: Once = Once::new();

    INIT_LOGGER.call_once(|| {
        let env = env_logger::Env::default().default_filter_or(default_filter);
        // Use a compact format with timestamps and targets to aid debugging.
        let mut builder = env_logger::Builder::from_env(env);
        builder.format_timestamp_micros().format_target(true);
        let _ = builder.try_init();
    });
}
