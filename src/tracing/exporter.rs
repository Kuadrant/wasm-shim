use crate::{WASM_SHIM_GIT_HASH, WASM_SHIM_NAME, WASM_SHIM_PROFILE, WASM_SHIM_VERSION};
use log::{debug, error, info};
use opentelemetry::KeyValue;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans};
use opentelemetry_sdk::{
    error::OTelSdkResult,
    trace::{SpanData, SpanExporter},
    Resource,
};
use prost::Message;
use std::{fmt::Debug, sync::OnceLock};

static WASM_SHIM_PROTO_RESOURCE: OnceLock<opentelemetry_proto::tonic::resource::v1::Resource> =
    OnceLock::new();

fn get_wasm_shim_proto_resource() -> &'static opentelemetry_proto::tonic::resource::v1::Resource {
    WASM_SHIM_PROTO_RESOURCE.get_or_init(|| {
        let resource = Resource::builder()
            .with_service_name(WASM_SHIM_NAME)
            .with_attributes([
                KeyValue::new("service.version", WASM_SHIM_VERSION),
                KeyValue::new("telemetry.sdk.name", "opentelemetry"),
                KeyValue::new("telemetry.sdk.language", "rust"),
                KeyValue::new("wasm.git_hash", WASM_SHIM_GIT_HASH),
                KeyValue::new("wasm.build_profile", WASM_SHIM_PROFILE),
            ])
            .build();

        opentelemetry_proto::tonic::resource::v1::Resource {
            attributes: resource
                .iter()
                .map(
                    |(key, value)| opentelemetry_proto::tonic::common::v1::KeyValue {
                        key: key.to_string(),
                        value: Some(value.clone().into()),
                    },
                )
                .collect(),
            dropped_attributes_count: 0,
            entity_refs: Vec::new(),
        }
    })
}

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
            if batch.is_empty() {
                return Ok(());
            }

            info!(
                "OTLP Exporter: Exporting {} spans to {}",
                batch.len(),
                endpoint
            );

            for span in &batch {
                debug!(
                    "  Span: name='{}' trace_id={:?} span_id={:?} parent={:?} status={:?}",
                    span.name,
                    span.span_context.trace_id(),
                    span.span_context.span_id(),
                    span.parent_span_id,
                    span.status
                );

                if !span.attributes.is_empty() {
                    log::trace!("    Attributes: {:?}", span.attributes);
                }

                if !span.events.is_empty() {
                    log::trace!("    Events: {:?}", span.events);
                }
            }

            let payload = match serialize_to_otlp(&batch) {
                Ok(data) => data,
                Err(e) => {
                    error!("Failed to serialize spans to OTLP: {}", e);
                    return Err(opentelemetry_sdk::error::OTelSdkError::InternalFailure(
                        format!("Failed to serialize spans: {}", e),
                    ));
                }
            };

            match dispatch_otlp_export(&endpoint, payload) {
                Ok(token_id) => {
                    info!("OTLP export dispatched with token_id: {}", token_id);
                    Ok(())
                }
                Err(e) => {
                    error!("Failed to dispatch OTLP export: {:?}", e);
                    Err(opentelemetry_sdk::error::OTelSdkError::InternalFailure(
                        format!("Failed to dispatch gRPC call: {:?}", e),
                    ))
                }
            }
        }
    }

    fn shutdown(&mut self) -> OTelSdkResult {
        debug!("Shutting down ProxyWasmOtlpExporter");
        Ok(())
    }
}

/// Serialize spans to OTLP protobuf format
fn serialize_to_otlp(batch: &[SpanData]) -> Result<Vec<u8>, String> {
    if batch.is_empty() {
        return Ok(Vec::new());
    }

    let spans: Vec<_> = batch
        .iter()
        .map(|span_data| span_data.clone().into())
        .collect();

    let scope_spans = ScopeSpans {
        scope: batch
            .first()
            .map(|s| (&s.instrumentation_scope, None).into()),
        spans,
        schema_url: String::new(),
    };

    let resource_spans = vec![ResourceSpans {
        resource: Some(get_wasm_shim_proto_resource().clone()),
        scope_spans: vec![scope_spans],
        schema_url: String::new(),
    }];

    let request = ExportTraceServiceRequest { resource_spans };

    let mut buf = Vec::new();
    request
        .encode(&mut buf)
        .map_err(|e| format!("Failed to encode OTLP request: {}", e))?;

    debug!("Serialized {} spans to {} bytes", batch.len(), buf.len());
    Ok(buf)
}

/// Dispatch OTLP export via gRPC call
fn dispatch_otlp_export(cluster: &str, payload: Vec<u8>) -> Result<u32, proxy_wasm::types::Status> {
    use std::time::Duration;

    // If payload is empty, pass None to avoid ParseFailure error
    let payload_ref = if payload.is_empty() {
        None
    } else {
        Some(payload.as_slice())
    };

    debug!(
        "dispatch_grpc_call: cluster='{}', service='opentelemetry.proto.collector.trace.v1.TraceService', method='Export', payload_size={}",
        cluster,
        payload.len()
    );

    proxy_wasm::hostcalls::dispatch_grpc_call(
        cluster,
        "opentelemetry.proto.collector.trace.v1.TraceService",
        "Export",
        vec![],
        payload_ref,
        Duration::from_secs(5),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exporter_creation() {
        let exporter = ProxyWasmOtlpExporter::new("localhost:4318");
        assert_eq!(exporter.endpoint, "localhost:4318");
    }

    #[test]
    fn test_exporter_debug() {
        let exporter = ProxyWasmOtlpExporter::new("test:4318");
        let debug_str = format!("{:?}", exporter);
        assert!(debug_str.contains("ProxyWasmOtlpExporter"));
        assert!(debug_str.contains("test:4318"));
    }

    #[test]
    fn test_serialize_empty_batch() {
        let result = serialize_to_otlp(&[]);
        assert!(result.is_ok());
        assert!(result.unwrap_or_default().is_empty());
    }
}
