use once_cell::sync::OnceCell;

/// Metrics interface allowing pluggable sinks (default: no-op).
pub trait RecorderMetrics: Send + Sync {
    /// Record that an event stream was dropped for the provided reason.
    fn record_dropped_event(&self, _reason: &'static str) {}
    /// Record that tracing detached, optionally linked to an error code.
    fn record_detach(&self, _reason: &'static str, _error_code: Option<&str>) {}
    /// Record that a panic was caught and converted into an error.
    fn record_panic(&self, _label: &'static str) {}
}

struct NoopMetrics;

impl RecorderMetrics for NoopMetrics {}

static METRICS_SINK: OnceCell<Box<dyn RecorderMetrics>> = OnceCell::new();

fn metrics_sink() -> &'static dyn RecorderMetrics {
    METRICS_SINK
        .get_or_init(|| Box::new(NoopMetrics) as Box<dyn RecorderMetrics>)
        .as_ref()
}

/// Install a custom metrics sink. Intended for embedding or tests.
#[cfg_attr(not(test), allow(dead_code))]
pub fn install_metrics(metrics: Box<dyn RecorderMetrics>) -> Result<(), Box<dyn RecorderMetrics>> {
    METRICS_SINK.set(metrics)
}

/// Record that we abandoned a monitoring location (e.g., synthetic filename).
pub fn record_dropped_event(reason: &'static str) {
    metrics_sink().record_dropped_event(reason);
}

/// Record that we detached per-policy or due to unrecoverable failure.
pub fn record_detach(reason: &'static str, error_code: Option<&str>) {
    metrics_sink().record_detach(reason, error_code);
}

/// Record that we caught a panic at the FFI boundary.
pub fn record_panic(label: &'static str) {
    metrics_sink().record_panic(label);
}

#[cfg(test)]
pub mod test_support {
    use super::*;
    use once_cell::sync::OnceCell;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    pub struct CapturingMetrics {
        events: Arc<Mutex<Vec<MetricEvent>>>,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum MetricEvent {
        Dropped(&'static str),
        Detach(&'static str, Option<String>),
        Panic(&'static str),
    }

    impl CapturingMetrics {
        pub fn take(&self) -> Vec<MetricEvent> {
            let mut guard = self.events.lock().expect("metrics events lock");
            let events = guard.clone();
            guard.clear();
            events
        }
    }

    impl RecorderMetrics for CapturingMetrics {
        fn record_dropped_event(&self, reason: &'static str) {
            self.events
                .lock()
                .expect("metrics events lock")
                .push(MetricEvent::Dropped(reason));
        }

        fn record_detach(&self, reason: &'static str, error_code: Option<&str>) {
            self.events
                .lock()
                .expect("metrics events lock")
                .push(MetricEvent::Detach(
                    reason,
                    error_code.map(|s| s.to_string()),
                ));
        }

        fn record_panic(&self, label: &'static str) {
            self.events
                .lock()
                .expect("metrics events lock")
                .push(MetricEvent::Panic(label));
        }
    }

    static CAPTURING: OnceCell<CapturingMetrics> = OnceCell::new();

    pub fn install() -> &'static CapturingMetrics {
        CAPTURING.get_or_init(|| {
            let metrics = CapturingMetrics::default();
            let _ = super::install_metrics(Box::new(metrics.clone()));
            metrics
        })
    }
}
