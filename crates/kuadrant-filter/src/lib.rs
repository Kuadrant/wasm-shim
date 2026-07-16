pub mod configuration;
pub mod data;
pub mod descriptor_manager;
pub mod kuadrant;
#[allow(unused_imports)]
mod proto;
pub mod services;
pub mod tracing;

#[macro_export]
macro_rules! record_error {
    ($($arg:tt)*) => {{
        use ::tracing::field;
        ::tracing::error!($($arg)*);
        let span = ::tracing::Span::current();
        span.record("otel.status_code", "ERROR");
        span.record("otel.status_message", &field::display(format_args!($($arg)*)));
    }};
}

pub mod metrics {
    use std::sync::Arc;

    pub trait MetricsReporter: Send + Sync {
        fn increment_denied(&self);
        fn increment_errors(&self);
    }

    pub struct NoopMetricsReporter;

    impl MetricsReporter for NoopMetricsReporter {
        fn increment_denied(&self) {}
        fn increment_errors(&self) {}
    }

    pub fn noop_metrics() -> Arc<dyn MetricsReporter> {
        Arc::new(NoopMetricsReporter)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum GrpcStatus {
    Ok = 0,
}
