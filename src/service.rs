pub(crate) mod auth;
pub(crate) mod rate_limit;

use crate::configuration::ExtensionType;
use crate::filter::http_context::TracingHeader;
use crate::service::auth::{AUTH_METHOD_NAME, AUTH_SERVICE_NAME};
use crate::service::rate_limit::{RATELIMIT_METHOD_NAME, RATELIMIT_SERVICE_NAME};
use protobuf::Message;
use proxy_wasm::hostcalls;
use proxy_wasm::hostcalls::dispatch_grpc_call;
use proxy_wasm::types::{Bytes, MapType, Status};
use std::cell::OnceCell;
use std::time::Duration;

pub struct GrpcServiceHandler {
    endpoint: String,
    service_name: String,
    method_name: String,
    tracing_headers: Vec<(TracingHeader, Bytes)>,
}

impl GrpcServiceHandler {
    fn build(
        endpoint: String,
        service_name: &str,
        method_name: &str,
        tracing_headers: Vec<(TracingHeader, Bytes)>,
    ) -> Self {
        Self {
            endpoint: endpoint.to_owned(),
            service_name: service_name.to_owned(),
            method_name: method_name.to_owned(),
            tracing_headers,
        }
    }

    pub fn new(
        extension_type: ExtensionType,
        endpoint: String,
        tracing_headers: Vec<(TracingHeader, Bytes)>,
    ) -> Self {
        match extension_type {
            ExtensionType::Auth => Self::build(
                endpoint,
                AUTH_SERVICE_NAME,
                AUTH_METHOD_NAME,
                tracing_headers,
            ),
            ExtensionType::RateLimit => Self::build(
                endpoint,
                RATELIMIT_SERVICE_NAME,
                RATELIMIT_METHOD_NAME,
                tracing_headers,
            ),
        }
    }

    pub fn send<M: Message>(&self, message: M) -> Result<u32, Status> {
        let msg = Message::write_to_bytes(&message).unwrap();
        let metadata = self
            .tracing_headers
            .iter()
            .map(|(header, value)| (header.as_str(), value.as_slice()))
            .collect();

        dispatch_grpc_call(
            self.endpoint.as_str(),
            self.service_name.as_str(),
            self.method_name.as_str(),
            metadata,
            Some(&msg),
            Duration::from_secs(5),
        )
    }
}

pub struct TracingHeaderResolver {
    tracing_headers: OnceCell<Vec<(TracingHeader, Bytes)>>,
}

impl TracingHeaderResolver {
    pub fn get(&self) -> &Vec<(TracingHeader, Bytes)> {
        self.tracing_headers.get_or_init(|| {
            let mut headers = Vec::new();
            for header in TracingHeader::all() {
                if let Some(value) =
                    hostcalls::get_map_value_bytes(MapType::HttpRequestHeaders, header.as_str())
                        .unwrap()
                {
                    headers.push((header, value));
                }
            }
            headers
        })
    }
}
