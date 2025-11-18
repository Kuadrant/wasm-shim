use opentelemetry_sdk::{
    error::OTelSdkResult,
    trace::{SpanData, SpanExporter},
};
use std::fmt::Debug;

pub struct ProxyWasmOtlpExporter {
    endpoint: String,
}

impl ProxyWasmOtlpExporter {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
        }
    }
}

impl Debug for ProxyWasmOtlpExporter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyWasmOtlpExporter")
            .field("endpoint", &self.endpoint)
            .finish()
    }
}

impl SpanExporter for ProxyWasmOtlpExporter {
    fn export(
        &self,
        batch: Vec<SpanData>,
    ) -> impl std::future::Future<Output = OTelSdkResult> + Send {
        let endpoint = self.endpoint.clone();
        async move {
            log::info!(
                "OTLP Exporter: Received {} spans for export to {}",
                batch.len(),
                endpoint
            );

            // Log each span's details for debugging
            for span in &batch {
                log::info!(
                    "  Span: name='{}' trace_id={:?} span_id={:?} parent={:?} status={:?}",
                    span.name,
                    span.span_context.trace_id(),
                    span.span_context.span_id(),
                    span.parent_span_id,
                    span.status
                );

                // Log span attributes
                if !span.attributes.is_empty() {
                    log::debug!("    Attributes: {:?}", span.attributes);
                }

                // Log events
                if !span.events.is_empty() {
                    log::debug!("    Events: {:?}", span.events);
                }
            }

            // TODO: Serialize batch to OTLP protobuf format
            // TODO: Dispatch HTTP call via proxy-wasm hostcalls

            Ok(())
        }
    }

    fn shutdown(&mut self) -> OTelSdkResult {
        log::debug!("Shutting down ProxyWasmOtlpExporter");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exporter_creation() {
        let exporter = ProxyWasmOtlpExporter::new("http://localhost:4318");
        assert_eq!(exporter.endpoint, "http://localhost:4318");
    }

    #[test]
    fn test_exporter_debug() {
        let exporter = ProxyWasmOtlpExporter::new("http://test:4318");
        let debug_str = format!("{:?}", exporter);
        assert!(debug_str.contains("ProxyWasmOtlpExporter"));
        assert!(debug_str.contains("http://test:4318"));
    }
}
