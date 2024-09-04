pub(crate) mod auth;
pub(crate) mod rate_limit;

use crate::configuration::{Extension, ExtensionType, FailureMode};
use crate::envoy::{CheckRequest, RateLimitDescriptor, RateLimitRequest};
use crate::service::auth::{AuthService, AUTH_METHOD_NAME, AUTH_SERVICE_NAME};
use crate::service::rate_limit::{RateLimitService, RATELIMIT_METHOD_NAME, RATELIMIT_SERVICE_NAME};
use crate::service::TracingHeader::{Baggage, Traceparent, Tracestate};
use protobuf::reflect::MessageDescriptor;
use protobuf::{
    Clear, CodedInputStream, CodedOutputStream, Message, ProtobufResult, UnknownFields,
};
use proxy_wasm::types::{Bytes, MapType, Status};
use std::any::Any;
use std::cell::OnceCell;
use std::fmt::Debug;
use std::rc::Rc;
use std::time::Duration;

#[derive(Clone, Debug)]
pub enum GrpcMessage {
    Auth(CheckRequest),
    RateLimit(RateLimitRequest),
}

impl Default for GrpcMessage {
    fn default() -> Self {
        GrpcMessage::RateLimit(RateLimitRequest::new())
    }
}

impl Clear for GrpcMessage {
    fn clear(&mut self) {
        match self {
            GrpcMessage::Auth(msg) => msg.clear(),
            GrpcMessage::RateLimit(msg) => msg.clear(),
        }
    }
}

impl Message for GrpcMessage {
    fn descriptor(&self) -> &'static MessageDescriptor {
        match self {
            GrpcMessage::Auth(msg) => msg.descriptor(),
            GrpcMessage::RateLimit(msg) => msg.descriptor(),
        }
    }

    fn is_initialized(&self) -> bool {
        match self {
            GrpcMessage::Auth(msg) => msg.is_initialized(),
            GrpcMessage::RateLimit(msg) => msg.is_initialized(),
        }
    }

    fn merge_from(&mut self, is: &mut CodedInputStream) -> ProtobufResult<()> {
        match self {
            GrpcMessage::Auth(msg) => msg.merge_from(is),
            GrpcMessage::RateLimit(msg) => msg.merge_from(is),
        }
    }

    fn write_to_with_cached_sizes(&self, os: &mut CodedOutputStream) -> ProtobufResult<()> {
        match self {
            GrpcMessage::Auth(msg) => msg.write_to_with_cached_sizes(os),
            GrpcMessage::RateLimit(msg) => msg.write_to_with_cached_sizes(os),
        }
    }

    fn write_to_bytes(&self) -> ProtobufResult<Vec<u8>> {
        match self {
            GrpcMessage::Auth(msg) => msg.write_to_bytes(),
            GrpcMessage::RateLimit(msg) => msg.write_to_bytes(),
        }
    }

    fn compute_size(&self) -> u32 {
        match self {
            GrpcMessage::Auth(msg) => msg.compute_size(),
            GrpcMessage::RateLimit(msg) => msg.compute_size(),
        }
    }

    fn get_cached_size(&self) -> u32 {
        match self {
            GrpcMessage::Auth(msg) => msg.get_cached_size(),
            GrpcMessage::RateLimit(msg) => msg.get_cached_size(),
        }
    }

    fn get_unknown_fields(&self) -> &UnknownFields {
        match self {
            GrpcMessage::Auth(msg) => msg.get_unknown_fields(),
            GrpcMessage::RateLimit(msg) => msg.get_unknown_fields(),
        }
    }

    fn mut_unknown_fields(&mut self) -> &mut UnknownFields {
        match self {
            GrpcMessage::Auth(msg) => msg.mut_unknown_fields(),
            GrpcMessage::RateLimit(msg) => msg.mut_unknown_fields(),
        }
    }

    fn as_any(&self) -> &dyn Any {
        match self {
            GrpcMessage::Auth(msg) => msg.as_any(),
            GrpcMessage::RateLimit(msg) => msg.as_any(),
        }
    }

    fn new() -> Self
    where
        Self: Sized,
    {
        // Returning default value
        GrpcMessage::default()
    }

    fn default_instance() -> &'static Self
    where
        Self: Sized,
    {
        #[allow(non_upper_case_globals)]
        static instance: ::protobuf::rt::LazyV2<GrpcMessage> = ::protobuf::rt::LazyV2::INIT;
        instance.get(|| GrpcMessage::RateLimit(RateLimitRequest::new()))
    }
}

impl GrpcMessage {
    // Using domain as ce_host for the time being, we might pass a DataType in the future.
    pub fn new(
        extension_type: ExtensionType,
        domain: String,
        descriptors: protobuf::RepeatedField<RateLimitDescriptor>,
    ) -> Self {
        match extension_type {
            ExtensionType::RateLimit => {
                GrpcMessage::RateLimit(RateLimitService::message(domain.clone(), descriptors))
            }
            ExtensionType::Auth => GrpcMessage::Auth(AuthService::message(domain.clone())),
        }
    }
}

#[derive(Default)]
pub struct GrpcService {
    #[allow(dead_code)]
    extension: Rc<Extension>,
    name: &'static str,
    method: &'static str,
}

impl GrpcService {
    pub fn new(extension: Rc<Extension>) -> Self {
        match extension.extension_type {
            ExtensionType::Auth => Self {
                extension,
                name: AUTH_SERVICE_NAME,
                method: AUTH_METHOD_NAME,
            },
            ExtensionType::RateLimit => Self {
                extension,
                name: RATELIMIT_SERVICE_NAME,
                method: RATELIMIT_METHOD_NAME,
            },
        }
    }

    fn endpoint(&self) -> &str {
        &self.extension.endpoint
    }
    fn name(&self) -> &str {
        self.name
    }
    fn method(&self) -> &str {
        self.method
    }
    pub fn failure_mode(&self) -> &FailureMode {
        &self.extension.failure_mode
    }
}

pub type GrpcCall = fn(
    upstream_name: &str,
    service_name: &str,
    method_name: &str,
    initial_metadata: Vec<(&str, &[u8])>,
    message: Option<&[u8]>,
    timeout: Duration,
) -> Result<u32, Status>;

pub type GetMapValuesBytes = fn(map_type: MapType, key: &str) -> Result<Option<Bytes>, Status>;

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

    pub fn send(
        &self,
        get_map_values_bytes: GetMapValuesBytes,
        grpc_call: GrpcCall,
        message: GrpcMessage,
    ) -> Result<u32, Status> {
        let msg = Message::write_to_bytes(&message).unwrap();
        let metadata = self
            .header_resolver
            .get(get_map_values_bytes)
            .iter()
            .map(|(header, value)| (*header, value.as_slice()))
            .collect();

        grpc_call(
            self.service.endpoint(),
            self.service.name(),
            self.service.method(),
            metadata,
            Some(&msg),
            Duration::from_secs(5),
        )
    }

    pub fn get_extension(&self) -> Rc<Extension> {
        Rc::clone(&self.service.extension)
    }

    pub fn get_extension_type(&self) -> ExtensionType {
        self.service.extension.extension_type.clone()
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

    pub fn get(&self, get_map_values_bytes: GetMapValuesBytes) -> &Vec<(&'static str, Bytes)> {
        self.headers.get_or_init(|| {
            let mut headers = Vec::new();
            for header in TracingHeader::all() {
                if let Ok(Some(value)) =
                    get_map_values_bytes(MapType::HttpRequestHeaders, (*header).as_str())
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
