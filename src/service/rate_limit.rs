use crate::envoy::{RateLimitDescriptor, RateLimitRequest};
use crate::service::errors::BuildMessageError;
use prost::Message;

pub const RATELIMIT_SERVICE_NAME: &str = "envoy.service.ratelimit.v3.RateLimitService";
pub const RATELIMIT_METHOD_NAME: &str = "ShouldRateLimit";

pub const KUADRANT_RATELIMIT_SERVICE_NAME: &str = "kuadrant.service.ratelimit.v1.RateLimitService";
pub const KUADRANT_CHECK_RATELIMIT_METHOD_NAME: &str = "CheckRateLimit";
pub const KUADRANT_REPORT_RATELIMIT_METHOD_NAME: &str = "Report";

pub struct RateLimitService;

impl RateLimitService {
    pub fn request_message(
        domain: String,
        descriptors: Vec<RateLimitDescriptor>,
        hits_addend: u32,
    ) -> RateLimitRequest {
        RateLimitRequest {
            domain,
            descriptors,
            hits_addend,
        }
    }

    pub fn request_message_as_bytes(
        domain: String,
        descriptors: Vec<RateLimitDescriptor>,
        hits_addend: u32,
    ) -> Result<Vec<u8>, BuildMessageError> {
        Ok(Self::request_message(domain, descriptors, hits_addend).encode_to_vec())
    }
}

#[cfg(test)]
mod tests {
    use crate::envoy::{rate_limit_descriptor, RateLimitDescriptor, RateLimitRequest};
    use crate::service::rate_limit::RateLimitService;

    fn build_message() -> RateLimitRequest {
        let domain = "rlp1";
        let mut field = RateLimitDescriptor::default();
        let entry = rate_limit_descriptor::Entry {
            key: "key1".to_string(),
            value: "value1".to_string(),
        };
        field.entries = vec![entry];
        let descriptors = vec![field];

        RateLimitService::request_message(domain.to_string(), descriptors, 1)
    }
    #[test]
    fn builds_correct_message() {
        let msg = build_message();

        assert_eq!(msg.hits_addend, 1);
        assert_eq!(msg.domain, "rlp1".to_string());
        assert_eq!(
            msg.descriptors
                .first()
                .expect("must have a descriptor")
                .entries[0]
                .key,
            "key1"
        );
        assert_eq!(
            msg.descriptors
                .first()
                .expect("must have a descriptor")
                .entries[0]
                .value,
            "value1"
        );
    }
}
