use std::time::Duration;

use prost::Message;
use tracing::{debug, info};

use super::{Service, ServiceError};
use crate::kuadrant::ReqRespCtx;
use crate::{WASM_SHIM_GIT_HASH, WASM_SHIM_NAME, WASM_SHIM_PROFILE, WASM_SHIM_VERSION};
use opentelemetry::KeyValue;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceResponse;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans};
use opentelemetry_sdk::{trace::SpanData, Resource};
use std::sync::OnceLock;

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

pub struct TracingService {
    upstream_name: String,
    service_name: String,
    method: String,
    timeout: Duration,
}

impl Service for TracingService {
    type Response = ExportTraceServiceResponse;

    fn parse_message(&self, message: Vec<u8>) -> Result<Self::Response, ServiceError> {
        prost::Message::decode(&message[..])
            .map_err(|e| ServiceError::Decode(format!("ExportTraceServiceResponse: {e}")))
    }
}

impl TracingService {
    pub fn new(endpoint: String, timeout: Duration) -> Self {
        Self {
            upstream_name: endpoint,
            service_name: "opentelemetry.proto.collector.trace.v1.TraceService".to_string(),
            method: "Export".to_string(),
            timeout,
        }
    }

    pub fn dispatch_export(
        &self,
        ctx: &mut ReqRespCtx,
        spans: &[SpanData],
    ) -> Result<u32, ServiceError> {
        if spans.is_empty() {
            return Err(ServiceError::Dispatch(
                "Cannot export empty span batch".to_string(),
            ));
        }

        info!(
            "OTLP Service: Exporting {} spans to {}",
            spans.len(),
            self.upstream_name
        );

        let request = self.build_export_request(spans)?;
        let outgoing_message = request.encode_to_vec();

        debug!(
            "Serialized {} spans to {} bytes",
            spans.len(),
            outgoing_message.len()
        );

        self.dispatch(
            ctx,
            &self.upstream_name,
            &self.service_name,
            &self.method,
            outgoing_message,
            self.timeout,
        )
    }

    fn build_export_request(
        &self,
        batch: &[SpanData],
    ) -> Result<ExportTraceServiceRequest, ServiceError> {
        if batch.is_empty() {
            return Ok(ExportTraceServiceRequest {
                resource_spans: Vec::new(),
            });
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

        Ok(ExportTraceServiceRequest { resource_spans })
    }
}
