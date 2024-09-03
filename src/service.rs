pub(crate) mod auth;
pub(crate) mod rate_limit;

use crate::configuration::{ExtensionType, FailureMode};
use crate::envoy::{RateLimitDescriptor, RateLimitRequest};
use crate::service::auth::{AUTH_METHOD_NAME, AUTH_SERVICE_NAME};
use crate::service::rate_limit::{RateLimitService, RATELIMIT_METHOD_NAME, RATELIMIT_SERVICE_NAME};
use crate::service::TracingHeader::{Baggage, Traceparent, Tracestate};
use protobuf::Message;
use proxy_wasm::hostcalls;
use proxy_wasm::hostcalls::dispatch_grpc_call;
use proxy_wasm::types::{Bytes, MapType, Status};
use std::cell::OnceCell;
use std::rc::Rc;
use std::time::Duration;

#[derive(Clone)]
pub enum GrpcMessage {
    //Auth(CheckRequest),
    RateLimit(RateLimitRequest),
}

impl GrpcMessage {
    pub fn get_message(&self) -> &RateLimitRequest {
        //TODO(didierofrivia): Should return Message
        match self {
            GrpcMessage::RateLimit(message) => message,
        }
    }
}

#[derive(Default)]
pub struct GrpcService {
    endpoint: String,
    #[allow(dead_code)]
    extension_type: ExtensionType,
    name: &'static str,
    method: &'static str,
    failure_mode: FailureMode,
}

impl GrpcService {
    pub fn new(extension_type: ExtensionType, endpoint: String, failure_mode: FailureMode) -> Self {
        match extension_type {
            ExtensionType::Auth => Self {
                endpoint,
                extension_type,
                name: AUTH_SERVICE_NAME,
                method: AUTH_METHOD_NAME,
                failure_mode,
            },
            ExtensionType::RateLimit => Self {
                endpoint,
                extension_type,
                name: RATELIMIT_SERVICE_NAME,
                method: RATELIMIT_METHOD_NAME,
                failure_mode,
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
    pub fn failure_mode(&self) -> &FailureMode {
        &self.failure_mode
    }
}

type GrpcCall = fn(
    upstream_name: &str,
    service_name: &str,
    method_name: &str,
    initial_metadata: Vec<(&str, &[u8])>,
    message: Option<&[u8]>,
    timeout: Duration,
) -> Result<u32, Status>;

pub struct GrpcServiceHandler {
    service: Rc<GrpcService>,
    header_resolver: Rc<HeaderResolver>,
    grpc_call: GrpcCall,
}

impl GrpcServiceHandler {
    pub fn new(
        service: Rc<GrpcService>,
        header_resolver: Rc<HeaderResolver>,
        grpc_call: Option<GrpcCall>,
    ) -> Self {
        Self {
            service,
            header_resolver,
            grpc_call: grpc_call.unwrap_or(dispatch_grpc_call),
        }
    }

    pub fn send(&self, message: GrpcMessage) -> Result<u32, Status> {
        let msg = Message::write_to_bytes(message.get_message()).unwrap();
        let metadata = self
            .header_resolver
            .get()
            .iter()
            .map(|(header, value)| (*header, value.as_slice()))
            .collect();

        (self.grpc_call)(
            self.service.endpoint(),
            self.service.name(),
            self.service.method(),
            metadata,
            Some(&msg),
            Duration::from_secs(5),
        )
    }

    // Using domain as ce_host for the time being, we might pass a DataType in the future.
    //TODO(didierofrivia): Make it work with Message. for both Auth and RL
    pub fn build_message(
        &self,
        domain: String,
        descriptors: protobuf::RepeatedField<RateLimitDescriptor>,
    ) -> GrpcMessage {
        /*match self.service.extension_type {
            //ExtensionType::Auth => GrpcMessage::Auth(AuthService::message(domain.clone())),
            //ExtensionType::RateLimit => GrpcMessage::RateLimit(RateLimitService::message(domain.clone(), descriptors)),
        }*/
        GrpcMessage::RateLimit(RateLimitService::message(domain.clone(), descriptors))
    }
}

pub struct HeaderResolver {
    headers: OnceCell<Vec<(&'static str, Bytes)>>,
}

impl Default for HeaderResolver {
    fn default() -> Self {
        Self::new()
    }
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
