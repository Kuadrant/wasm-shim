mod log_layer;
mod processor;
mod propagation;

pub use processor::{get_span_processor, BufferingSpanProcessor};
pub use propagation::{HeadersExtractor, HeadersInjector};

use log_layer::LogLayer;
use opentelemetry::trace::TracerProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use std::sync::OnceLock;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{reload, Layer};

type OtelFilterHandle = reload::Handle<Option<LevelFilter>, tracing_subscriber::Registry>;

type LogFilterHandle = reload::Handle<
    LevelFilter,
    tracing_subscriber::layer::Layered<
        tracing_subscriber::filter::Filtered<
            tracing_opentelemetry::OpenTelemetryLayer<
                tracing_subscriber::Registry,
                opentelemetry_sdk::trace::Tracer,
            >,
            reload::Layer<Option<LevelFilter>, tracing_subscriber::Registry>,
            tracing_subscriber::Registry,
        >,
        tracing_subscriber::Registry,
    >,
>;

static OTEL_FILTER_HANDLE: OnceLock<OtelFilterHandle> = OnceLock::new();
static LOG_FILTER_HANDLE: OnceLock<LogFilterHandle> = OnceLock::new();

pub fn init_observability(use_tracing: bool, log_level: Option<&str>, initial_filter: LevelFilter) {
    let otel_filter = if use_tracing {
        Some(match log_level {
            Some("TRACE") => LevelFilter::TRACE,
            Some("DEBUG") => LevelFilter::DEBUG,
            Some("INFO") => LevelFilter::INFO,
            Some("WARN") => LevelFilter::WARN,
            Some("ERROR") => LevelFilter::ERROR,
            Some("OFF") => LevelFilter::OFF,
            _ => LevelFilter::WARN,
        })
    } else {
        None
    };

    if let Some(otel_handle) = OTEL_FILTER_HANDLE.get() {
        // Handle already set, reload the layer
        if let Err(e) = otel_handle.reload(otel_filter) {
            log::error!("Failed to reload OpenTelemetry filter: {:?}", e);
        }
        set_log_level(initial_filter);
    } else {
        // Initialise global tracing subscriber and store handles to the filters
        let processor_handle = processor::SpanProcessorHandle;
        let provider = SdkTracerProvider::builder()
            .with_span_processor(processor_handle)
            .build();
        let tracer = provider.tracer("wasm-shim");

        let (otel_filter_layer, otel_filter_handle) = reload::Layer::new(otel_filter);
        let (log_filter_layer, log_filter_handle) = reload::Layer::new(initial_filter);

        let _ = tracing_subscriber::registry()
            .with(
                tracing_opentelemetry::layer()
                    .with_tracer(tracer)
                    .with_filter(otel_filter_layer),
            )
            .with(LogLayer.with_filter(log_filter_layer))
            .try_init();

        let _ = OTEL_FILTER_HANDLE.set(otel_filter_handle);
        let _ = LOG_FILTER_HANDLE.set(log_filter_handle);
    }
}

pub fn set_log_level(filter: LevelFilter) {
    if let Some(log_handle) = LOG_FILTER_HANDLE.get() {
        if let Err(e) = log_handle.reload(filter) {
            log::error!("Failed to reload LogLayer filter: {:?}", e);
        }
    }
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
