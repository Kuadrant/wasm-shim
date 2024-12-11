use crate::configuration::FailureMode;
use crate::envoy::{
    RateLimitDescriptor, RateLimitRequest, RateLimitResponse, RateLimitResponse_Code, StatusCode,
};
use crate::service::grpc_message::{GrpcMessageResponse, GrpcMessageResult};
use crate::service::{GrpcResult, GrpcService};
use log::{debug, warn};
use protobuf::{Message, RepeatedField};
use proxy_wasm::hostcalls;
use proxy_wasm::types::Bytes;

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

    pub fn response_message(res_body_bytes: &Bytes) -> GrpcMessageResult<GrpcMessageResponse> {
        match Message::parse_from_bytes(res_body_bytes) {
            Ok(res) => Ok(GrpcMessageResponse::RateLimit(res)),
            Err(e) => Err(e),
        }
    }

    pub fn process_ratelimit_grpc_response(
        rl_resp: GrpcMessageResponse,
        failure_mode: FailureMode,
    ) -> Result<GrpcResult, StatusCode> {
        match rl_resp {
            GrpcMessageResponse::RateLimit(RateLimitResponse {
                overall_code: RateLimitResponse_Code::UNKNOWN,
                ..
            }) => {
                GrpcService::handle_error_on_grpc_response(failure_mode);
                Err(StatusCode::InternalServerError)
            }
            GrpcMessageResponse::RateLimit(RateLimitResponse {
                overall_code: RateLimitResponse_Code::OVER_LIMIT,
                response_headers_to_add: rl_headers,
                ..
            }) => {
                let mut response_headers = vec![];
                for header in &rl_headers {
                    response_headers.push((header.get_key(), header.get_value()));
                }
                hostcalls::send_http_response(429, response_headers, Some(b"Too Many Requests\n"))
                    .expect("failed to send_http_response 429 while OVER_LIMIT");
                Err(StatusCode::TooManyRequests)
            }
            GrpcMessageResponse::RateLimit(RateLimitResponse {
                overall_code: RateLimitResponse_Code::OK,
                response_headers_to_add: additional_headers,
                ..
            }) => {
                let result = GrpcResult::new(
                    additional_headers
                        .iter()
                        .map(|header| (header.get_key().to_owned(), header.get_value().to_owned()))
                        .collect(),
                );
                Ok(result)
            }
            _ => {
                warn!("not a valid GrpcMessageResponse::RateLimit(RateLimitResponse)!");
                GrpcService::handle_error_on_grpc_response(failure_mode);
                Err(StatusCode::InternalServerError)
            }
        }
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
