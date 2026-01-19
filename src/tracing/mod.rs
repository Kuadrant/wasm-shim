mod log_layer;
mod processor;
mod propagation;

pub use processor::{get_span_processor, BufferingSpanProcessor};
pub use propagation::{HeadersExtractor, HeadersInjector};

use log_layer::LogLayer;
use opentelemetry::trace::TracerProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use std::sync::OnceLock;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

static TRACING_INITIALIZED: OnceLock<()> = OnceLock::new();

pub fn init_tracing(use_exporter: bool, log_level: Option<&str>) {
    TRACING_INITIALIZED.get_or_init(|| {
        let filter = match log_level {
            Some("TRACE") => tracing_subscriber::filter::LevelFilter::TRACE,
            Some("DEBUG") => tracing_subscriber::filter::LevelFilter::DEBUG,
            Some("INFO") => tracing_subscriber::filter::LevelFilter::INFO,
            Some("WARN") => tracing_subscriber::filter::LevelFilter::WARN,
            Some("ERROR") => tracing_subscriber::filter::LevelFilter::ERROR,
            Some("OFF") => tracing_subscriber::filter::LevelFilter::OFF,
            _ => tracing_subscriber::filter::LevelFilter::WARN,
        };

        if use_exporter {
            let processor_handle = processor::SpanProcessorHandle;

            let provider = SdkTracerProvider::builder()
                .with_span_processor(processor_handle)
                .build();

            let tracer = provider.tracer("wasm-shim");

            let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
            let _ = tracing_subscriber::registry()
                .with(filter)
                .with(otel_layer)
                .try_init();
        } else {
            let log_layer = LogLayer;
            let _ = tracing_subscriber::registry()
                .with(filter)
                .with(log_layer)
                .try_init();
        }
    });
}

/// Records an error on the current span and logs it.
#[macro_export]
macro_rules! record_error {
    ($($arg:tt)*) => {{
        use tracing::field;
        tracing::error!($($arg)*);
        let span = tracing::Span::current();
        span.record("otel.status_code", "ERROR");
        span.record("otel.status_message", &field::display(format_args!($($arg)*)));
    }};
}
