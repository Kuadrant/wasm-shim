use crate::configuration::ExtensionType;
use crate::envoy::{
    CheckRequest, DeniedHttpResponse, OkHttpResponse, RateLimitDescriptor, RateLimitRequest,
    RateLimitResponse,
};
use crate::service::auth::AuthService;
use crate::service::rate_limit::RateLimitService;
use protobuf::reflect::MessageDescriptor;
use protobuf::{
    Clear, CodedInputStream, CodedOutputStream, Message, ProtobufError, ProtobufResult,
    UnknownFields,
};
use proxy_wasm::types::Bytes;
use std::any::Any;

#[derive(Clone, Debug)]
pub enum GrpcMessageRequest {
    Auth(CheckRequest),
    RateLimit(RateLimitRequest),
}

impl Default for GrpcMessageRequest {
    fn default() -> Self {
        GrpcMessageRequest::RateLimit(RateLimitRequest::new())
    }
}

impl Clear for GrpcMessageRequest {
    fn clear(&mut self) {
        match self {
            GrpcMessageRequest::Auth(msg) => msg.clear(),
            GrpcMessageRequest::RateLimit(msg) => msg.clear(),
        }
    }
}

impl Message for GrpcMessageRequest {
    fn descriptor(&self) -> &'static MessageDescriptor {
        match self {
            GrpcMessageRequest::Auth(msg) => msg.descriptor(),
            GrpcMessageRequest::RateLimit(msg) => msg.descriptor(),
        }
    }

    fn is_initialized(&self) -> bool {
        match self {
            GrpcMessageRequest::Auth(msg) => msg.is_initialized(),
            GrpcMessageRequest::RateLimit(msg) => msg.is_initialized(),
        }
    }

    fn merge_from(&mut self, is: &mut CodedInputStream) -> ProtobufResult<()> {
        match self {
            GrpcMessageRequest::Auth(msg) => msg.merge_from(is),
            GrpcMessageRequest::RateLimit(msg) => msg.merge_from(is),
        }
    }

    fn write_to_with_cached_sizes(&self, os: &mut CodedOutputStream) -> ProtobufResult<()> {
        match self {
            GrpcMessageRequest::Auth(msg) => msg.write_to_with_cached_sizes(os),
            GrpcMessageRequest::RateLimit(msg) => msg.write_to_with_cached_sizes(os),
        }
    }

    fn write_to_bytes(&self) -> ProtobufResult<Vec<u8>> {
        match self {
            GrpcMessageRequest::Auth(msg) => msg.write_to_bytes(),
            GrpcMessageRequest::RateLimit(msg) => msg.write_to_bytes(),
        }
    }

    fn compute_size(&self) -> u32 {
        match self {
            GrpcMessageRequest::Auth(msg) => msg.compute_size(),
            GrpcMessageRequest::RateLimit(msg) => msg.compute_size(),
        }
    }

    fn get_cached_size(&self) -> u32 {
        match self {
            GrpcMessageRequest::Auth(msg) => msg.get_cached_size(),
            GrpcMessageRequest::RateLimit(msg) => msg.get_cached_size(),
        }
    }

    fn get_unknown_fields(&self) -> &UnknownFields {
        match self {
            GrpcMessageRequest::Auth(msg) => msg.get_unknown_fields(),
            GrpcMessageRequest::RateLimit(msg) => msg.get_unknown_fields(),
        }
    }

    fn mut_unknown_fields(&mut self) -> &mut UnknownFields {
        match self {
            GrpcMessageRequest::Auth(msg) => msg.mut_unknown_fields(),
            GrpcMessageRequest::RateLimit(msg) => msg.mut_unknown_fields(),
        }
    }

    fn as_any(&self) -> &dyn Any {
        match self {
            GrpcMessageRequest::Auth(msg) => msg.as_any(),
            GrpcMessageRequest::RateLimit(msg) => msg.as_any(),
        }
    }

    fn new() -> Self
    where
        Self: Sized,
    {
        // Returning default value
        GrpcMessageRequest::default()
    }

    fn default_instance() -> &'static Self
    where
        Self: Sized,
    {
        #[allow(non_upper_case_globals)]
        static instance: ::protobuf::rt::LazyV2<GrpcMessageRequest> = ::protobuf::rt::LazyV2::INIT;
        instance.get(|| GrpcMessageRequest::RateLimit(RateLimitRequest::new()))
    }
}

impl GrpcMessageRequest {
    // Using domain as ce_host for the time being, we might pass a DataType in the future.
    pub fn new(
        extension_type: ExtensionType,
        domain: String,
        descriptors: protobuf::RepeatedField<RateLimitDescriptor>,
    ) -> Self {
        match extension_type {
            ExtensionType::RateLimit => GrpcMessageRequest::RateLimit(
                RateLimitService::request_message(domain.clone(), descriptors),
            ),
            ExtensionType::Auth => {
                GrpcMessageRequest::Auth(AuthService::request_message(domain.clone()))
            }
        }
    }
}

#[derive(Clone, Debug)]
pub enum GrpcMessageResponse {
    AuthOk(OkHttpResponse),
    AuthDenied(DeniedHttpResponse),
    RateLimit(RateLimitResponse),
}

impl Default for GrpcMessageResponse {
    fn default() -> Self {
        GrpcMessageResponse::RateLimit(RateLimitResponse::new())
    }
}

impl Clear for GrpcMessageResponse {
    fn clear(&mut self) {
        todo!()
    }
}

impl Message for GrpcMessageResponse {
    fn descriptor(&self) -> &'static MessageDescriptor {
        match self {
            GrpcMessageResponse::AuthOk(res) => res.descriptor(),
            GrpcMessageResponse::AuthDenied(res) => res.descriptor(),
            GrpcMessageResponse::RateLimit(res) => res.descriptor(),
        }
    }

    fn is_initialized(&self) -> bool {
        match self {
            GrpcMessageResponse::AuthOk(res) => res.is_initialized(),
            GrpcMessageResponse::AuthDenied(res) => res.is_initialized(),
            GrpcMessageResponse::RateLimit(res) => res.is_initialized(),
        }
    }

    fn merge_from(&mut self, is: &mut CodedInputStream) -> ProtobufResult<()> {
        match self {
            GrpcMessageResponse::AuthOk(res) => res.merge_from(is),
            GrpcMessageResponse::AuthDenied(res) => res.merge_from(is),
            GrpcMessageResponse::RateLimit(res) => res.merge_from(is),
        }
    }

    fn write_to_with_cached_sizes(&self, os: &mut CodedOutputStream) -> ProtobufResult<()> {
        match self {
            GrpcMessageResponse::AuthOk(res) => res.write_to_with_cached_sizes(os),
            GrpcMessageResponse::AuthDenied(res) => res.write_to_with_cached_sizes(os),
            GrpcMessageResponse::RateLimit(res) => res.write_to_with_cached_sizes(os),
        }
    }

    fn write_to_bytes(&self) -> ProtobufResult<Vec<u8>> {
        match self {
            GrpcMessageResponse::AuthOk(res) => res.write_to_bytes(),
            GrpcMessageResponse::AuthDenied(res) => res.write_to_bytes(),
            GrpcMessageResponse::RateLimit(res) => res.write_to_bytes(),
        }
    }

    fn compute_size(&self) -> u32 {
        match self {
            GrpcMessageResponse::AuthOk(res) => res.compute_size(),
            GrpcMessageResponse::AuthDenied(res) => res.compute_size(),
            GrpcMessageResponse::RateLimit(res) => res.compute_size(),
        }
    }

    fn get_cached_size(&self) -> u32 {
        match self {
            GrpcMessageResponse::AuthOk(res) => res.get_cached_size(),
            GrpcMessageResponse::AuthDenied(res) => res.get_cached_size(),
            GrpcMessageResponse::RateLimit(res) => res.get_cached_size(),
        }
    }

    fn get_unknown_fields(&self) -> &UnknownFields {
        match self {
            GrpcMessageResponse::AuthOk(res) => res.get_unknown_fields(),
            GrpcMessageResponse::AuthDenied(res) => res.get_unknown_fields(),
            GrpcMessageResponse::RateLimit(res) => res.get_unknown_fields(),
        }
    }

    fn mut_unknown_fields(&mut self) -> &mut UnknownFields {
        match self {
            GrpcMessageResponse::AuthOk(res) => res.mut_unknown_fields(),
            GrpcMessageResponse::AuthDenied(res) => res.mut_unknown_fields(),
            GrpcMessageResponse::RateLimit(res) => res.mut_unknown_fields(),
        }
    }

    fn as_any(&self) -> &dyn Any {
        match self {
            GrpcMessageResponse::AuthOk(res) => res.as_any(),
            GrpcMessageResponse::AuthDenied(res) => res.as_any(),
            GrpcMessageResponse::RateLimit(res) => res.as_any(),
        }
    }

    fn new() -> Self
    where
        Self: Sized,
    {
        // Returning default value
        GrpcMessageResponse::default()
    }

    fn default_instance() -> &'static Self
    where
        Self: Sized,
    {
        #[allow(non_upper_case_globals)]
        static instance: ::protobuf::rt::LazyV2<GrpcMessageResponse> = ::protobuf::rt::LazyV2::INIT;
        instance.get(|| GrpcMessageResponse::RateLimit(RateLimitResponse::new()))
    }
}

impl GrpcMessageResponse {
    pub fn new(
        extension_type: &ExtensionType,
        res_body_bytes: &Bytes,
        status_code: u32,
    ) -> GrpcMessageResult<GrpcMessageResponse> {
        match extension_type {
            ExtensionType::RateLimit => RateLimitService::response_message(res_body_bytes),
            ExtensionType::Auth => AuthService::response_message(res_body_bytes, status_code),
        }
    }
}

pub type GrpcMessageResult<T> = Result<T, ProtobufError>;
