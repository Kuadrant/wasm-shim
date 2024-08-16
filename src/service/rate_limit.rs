use crate::envoy::{RateLimitDescriptor, RateLimitRequest};
use crate::filter::http_context::TracingHeader;
use crate::service::Service;
use protobuf::{Message, RepeatedField};
use proxy_wasm::hostcalls::dispatch_grpc_call;
use proxy_wasm::types::{Bytes, Status};
use std::time::Duration;

const RATELIMIT_SERVICE_NAME: &str = "envoy.service.ratelimit.v3.RateLimitService";
const RATELIMIT_METHOD_NAME: &str = "ShouldRateLimit";
pub struct RateLimitService {
    endpoint: String,
    tracing_headers: Vec<(TracingHeader, Bytes)>,
}

impl RateLimitService {
    pub fn new(endpoint: &str, metadata: Vec<(TracingHeader, Bytes)>) -> Self {
        Self {
            endpoint: String::from(endpoint),
            tracing_headers: metadata,
        }
    }

    pub fn message(
        domain: String,
        descriptors: RepeatedField<RateLimitDescriptor>,
    ) -> RateLimitRequest {
        RateLimitRequest {
            domain,
            descriptors,
            hits_addend: 1,
            unknown_fields: Default::default(),
            cached_size: Default::default(),
        }
    }
}

impl Service<RateLimitRequest> for RateLimitService {
    fn send(&self, message: RateLimitRequest) -> Result<u32, Status> {
        let msg = Message::write_to_bytes(&message).unwrap(); // TODO(didierofrivia): Error Handling
        let metadata = self
            .tracing_headers
            .iter()
            .map(|(header, value)| (header.as_str(), value.as_slice()))
            .collect();

        dispatch_grpc_call(
            self.endpoint.as_str(),
            RATELIMIT_SERVICE_NAME,
            RATELIMIT_METHOD_NAME,
            metadata,
            Some(&msg),
            Duration::from_secs(5),
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::envoy::{RateLimitDescriptor, RateLimitDescriptor_Entry, RateLimitRequest};
    use crate::service::rate_limit::RateLimitService;
    //use crate::service::Service;
    use protobuf::{CachedSize, RepeatedField, UnknownFields};
    //use proxy_wasm::types::Status;
    //use crate::filter::http_context::{Filter};

    fn build_message() -> RateLimitRequest {
        let domain = "rlp1";
        let mut field = RateLimitDescriptor::new();
        let mut entry = RateLimitDescriptor_Entry::new();
        entry.set_key("key1".to_string());
        entry.set_value("value1".to_string());
        field.set_entries(RepeatedField::from_vec(vec![entry]));
        let descriptors = RepeatedField::from_vec(vec![field]);

        RateLimitService::message(domain.to_string(), descriptors.clone())
    }
    #[test]
    fn builds_correct_message() {
        let msg = build_message();

        assert_eq!(msg.hits_addend, 1);
        assert_eq!(msg.domain, "rlp1".to_string());
        assert_eq!(msg.descriptors.first().unwrap().entries[0].key, "key1");
        assert_eq!(msg.descriptors.first().unwrap().entries[0].value, "value1");
        assert_eq!(msg.unknown_fields, UnknownFields::default());
        assert_eq!(msg.cached_size, CachedSize::default());
    }
    /*#[test]
    fn sends_message() {
        let msg = build_message();
        let metadata   = vec![("header-1", "value-1".as_bytes())];
        let rls = RateLimitService::new("limitador-cluster", metadata);

        // TODO(didierofrivia): When we have a grpc response type, assert the async response
    }

    fn grpc_call(
        _upstream_name: &str,
        _initial_metadata: Vec<(&str, &[u8])>,
        _message: RateLimitRequest,
    ) -> Result<u32, Status> {
        Ok(1)
    } */
}
