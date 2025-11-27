mod processor;
mod propagation;

pub use processor::{get_span_processor, BufferingSpanProcessor};
pub use propagation::{HeadersExtractor, HeadersInjector};

use opentelemetry::trace::TracerProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use std::sync::OnceLock;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

static TRACING_INITIALIZED: OnceLock<()> = OnceLock::new();

pub fn init_tracing(ctx: &mut crate::kuadrant::ReqRespCtx) {
    TRACING_INITIALIZED.get_or_init(|| {
        let processor_handle = processor::SpanProcessorHandle;

        let provider = SdkTracerProvider::builder()
            .with_span_processor(processor_handle)
            .build();

        let tracer = provider.tracer("wasm-shim");

        let _ = tracing_subscriber::registry()
            .with(tracing_opentelemetry::layer().with_tracer(tracer))
            .try_init();

        // Bridge log crate to tracing
        tracing_log::LogTracer::init().ok();
    });

    ctx.enter_request_span();
}
