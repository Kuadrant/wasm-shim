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

#[derive(Default)]
pub struct GrpcService {
    endpoint: String,
    name: &'static str,
    method: &'static str,
}

impl GrpcService {
    pub fn new(extension_type: ExtensionType, endpoint: String) -> Self {
        match extension_type {
            ExtensionType::Auth => Self {
                endpoint,
                name: AUTH_SERVICE_NAME,
                method: AUTH_METHOD_NAME,
            },
            ExtensionType::RateLimit => Self {
                endpoint,
                name: RATELIMIT_SERVICE_NAME,
                method: RATELIMIT_METHOD_NAME,
            },
        }
    }
    fn endpoint(&self) -> &str {
        &self.endpoint
    }
    fn name(&self) -> &str {
        self.name
    }
    fn method(&self) -> &str {
        self.method
    }
}

pub struct GrpcServiceHandler {
    service: Rc<GrpcService>,
    header_resolver: Rc<HeaderResolver>,
}

impl GrpcServiceHandler {
    pub fn new(service: Rc<GrpcService>, header_resolver: Rc<HeaderResolver>) -> Self {
        Self {
            service,
            header_resolver,
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
            self.service.endpoint(),
            self.service.name(),
            self.service.method(),
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
