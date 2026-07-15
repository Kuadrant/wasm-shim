mod processor;
mod propagation;

pub use processor::{get_span_processor, BufferingSpanProcessor, SpanProcessorHandle};
pub use propagation::{HeadersExtractor, HeadersInjector};
