pub(crate) mod auth;
pub(crate) mod rate_limit;

use crate::configuration::ExtensionType;
use crate::service::auth::{AUTH_METHOD_NAME, AUTH_SERVICE_NAME};
use crate::service::rate_limit::{RATELIMIT_METHOD_NAME, RATELIMIT_SERVICE_NAME};
use crate::service::TracingHeader::{Baggage, Traceparent, Tracestate};
use protobuf::Message;
use proxy_wasm::hostcalls;
use proxy_wasm::hostcalls::dispatch_grpc_call;
use proxy_wasm::types::{Bytes, MapType, Status};
use std::cell::OnceCell;
use std::rc::Rc;
use std::time::Duration;

pub struct GrpcServiceHandler {
    endpoint: String,
    service_name: String,
    method_name: String,
    header_resolver: Rc<HeaderResolver>,
}

impl GrpcServiceHandler {
    fn build(
        endpoint: String,
        service_name: &str,
        method_name: &str,
        header_resolver: Rc<HeaderResolver>,
    ) -> Self {
        Self {
            endpoint: endpoint.to_owned(),
            service_name: service_name.to_owned(),
            method_name: method_name.to_owned(),
            header_resolver,
        }
    }

    pub fn new(
        extension_type: ExtensionType,
        endpoint: String,
        header_resolver: Rc<HeaderResolver>,
    ) -> Self {
        match extension_type {
            ExtensionType::Auth => Self::build(
                endpoint,
                AUTH_SERVICE_NAME,
                AUTH_METHOD_NAME,
                header_resolver,
            ),
            ExtensionType::RateLimit => Self::build(
                endpoint,
                RATELIMIT_SERVICE_NAME,
                RATELIMIT_METHOD_NAME,
                header_resolver,
            ),
        }
    }

    pub fn send<M: Message>(&self, message: M) -> Result<u32, Status> {
        let msg = Message::write_to_bytes(&message).unwrap();
        let metadata = self
            .header_resolver
            .get()
            .iter()
            .map(|(header, value)| (*header, value.as_slice()))
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

pub struct HeaderResolver {
    headers: OnceCell<Vec<(&'static str, Bytes)>>,
}

impl HeaderResolver {
    pub fn new() -> Self {
        Self {
            headers: OnceCell::new(),
        }
    }

    pub fn get(&self) -> &Vec<(&'static str, Bytes)> {
        self.headers.get_or_init(|| {
            let mut headers = Vec::new();
            for header in TracingHeader::all() {
                if let Ok(Some(value)) =
                    hostcalls::get_map_value_bytes(MapType::HttpRequestHeaders, (*header).as_str())
                {
                    headers.push(((*header).as_str(), value));
                }
            }
            headers
        })
    }
}

// tracing headers
pub enum TracingHeader {
    Traceparent,
    Tracestate,
    Baggage,
}

impl TracingHeader {
    fn all() -> &'static [Self; 3] {
        &[Traceparent, Tracestate, Baggage]
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Traceparent => "traceparent",
            Tracestate => "tracestate",
            Baggage => "baggage",
        }
    }
}
