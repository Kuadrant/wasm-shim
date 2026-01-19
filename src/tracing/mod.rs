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

static RELOAD_HANDLES: OnceLock<(OtelFilterHandle, LogFilterHandle)> = OnceLock::new();

pub fn init_tracing(use_exporter: bool, log_level: Option<&str>) {
    let level = match log_level {
        Some("TRACE") => LevelFilter::TRACE,
        Some("DEBUG") => LevelFilter::DEBUG,
        Some("INFO") => LevelFilter::INFO,
        Some("WARN") => LevelFilter::WARN,
        Some("ERROR") => LevelFilter::ERROR,
        Some("OFF") => LevelFilter::OFF,
        _ => LevelFilter::WARN,
    };

    let (otel_filter, log_filter) = if use_exporter {
        (Some(level), LevelFilter::OFF)
    } else {
        (None, level)
    };

    if let Some((otel_handle, log_handle)) = RELOAD_HANDLES.get() {
        // handles already set, reload the layers
        if let Err(e) = otel_handle.reload(otel_filter) {
            log::error!("Failed to reload OpenTelemetry filter: {:?}", e);
        }
        if let Err(e) = log_handle.reload(log_filter) {
            log::error!("Failed to reload LogLayer filter: {:?}", e);
        }
    } else {
        // initialise global tracing subscriber and store handles to the filters
        let processor_handle = processor::SpanProcessorHandle;
        let provider = SdkTracerProvider::builder()
            .with_span_processor(processor_handle)
            .build();
        let tracer = provider.tracer("wasm-shim");

        let (otel_filter_layer, otel_filter_handle) = reload::Layer::new(otel_filter);
        let (log_filter_layer, log_filter_handle) = reload::Layer::new(log_filter);

        let _ = tracing_subscriber::registry()
            .with(
                tracing_opentelemetry::layer()
                    .with_tracer(tracer)
                    .with_filter(otel_filter_layer),
            )
            .with(LogLayer.with_filter(log_filter_layer))
            .try_init();

        let _ = RELOAD_HANDLES.set((otel_filter_handle, log_filter_handle));
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
