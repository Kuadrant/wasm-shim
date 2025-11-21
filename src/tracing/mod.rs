mod exporter;
mod processor;

pub use exporter::ProxyWasmOtlpExporter;
pub use processor::{get_span_processor, BufferingSpanProcessor};

use opentelemetry::trace::TracerProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;

pub fn otlp_layer<S>(endpoint: impl Into<String>) -> impl tracing_subscriber::Layer<S>
where
    S: ::tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    let exporter = ProxyWasmOtlpExporter::new(endpoint);

    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter)
        .build();

    let tracer = provider.tracer("wasm-shim");

    tracing_opentelemetry::layer().with_tracer(tracer)
}
