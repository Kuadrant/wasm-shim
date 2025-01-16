use crate::envoy::{RateLimitDescriptor, RateLimitRequest};
use log::debug;
use protobuf::{Message, RepeatedField};

pub const RATELIMIT_SERVICE_NAME: &str = "envoy.service.ratelimit.v3.RateLimitService";
pub const RATELIMIT_METHOD_NAME: &str = "ShouldRateLimit";

pub struct RateLimitService;

impl RateLimitService {
    pub fn request_message(
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

    pub fn request_message_as_bytes(
        domain: String,
        descriptors: RepeatedField<RateLimitDescriptor>,
    ) -> Option<Vec<u8>> {
        Self::request_message(domain, descriptors)
            .write_to_bytes()
            .map_err(|e| debug!("Failed to write protobuf message to bytes: {e:?}"))
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use crate::envoy::{RateLimitDescriptor, RateLimitDescriptor_Entry, RateLimitRequest};
    use crate::service::rate_limit::RateLimitService;
    //use crate::service::Service;
    use protobuf::{CachedSize, RepeatedField, UnknownFields};

    fn build_message() -> RateLimitRequest {
        let domain = "rlp1";
        let mut field = RateLimitDescriptor::new();
        let mut entry = RateLimitDescriptor_Entry::new();
        entry.set_key("key1".to_string());
        entry.set_value("value1".to_string());
        field.set_entries(RepeatedField::from_vec(vec![entry]));
        let descriptors = RepeatedField::from_vec(vec![field]);

        RateLimitService::request_message(domain.to_string(), descriptors.clone())
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
        assert_eq!(msg.unknown_fields, UnknownFields::default());
        assert_eq!(msg.cached_size, CachedSize::default());
    }
}
