use std::time::Duration;

use cel_interpreter::Value;
use prost::Message;

use crate::envoy::{
    rate_limit_descriptor, RateLimitDescriptor, RateLimitRequest, RateLimitResponse,
};
use crate::v2::data::attribute::AttributeState;
use crate::v2::{
    data::attribute::AttributeError,
    kuadrant::ReqRespCtx,
    services::{Service, ServiceError},
};

pub type RateLimitDescriptorData = Vec<(String, String)>;

struct RateLimitService {
    upstream_name: String,
    service_name: String,
    method: String,
    timeout: Duration,
}

impl Service for RateLimitService {
    type Response = RateLimitResponse;

    fn parse_message(&self, message: Vec<u8>) -> Result<Self::Response, ServiceError> {
        prost::Message::decode(&message[..])
            .map_err(|e| ServiceError::DecodeFailed(format!("RateLimitResponse: {e}")))
    }
}

impl RateLimitService {
    fn dispatch_ratelimit(
        &self,
        ctx: &mut ReqRespCtx,
        scope: &str,
        descriptors: Vec<RateLimitDescriptorData>,
        hits_addend: u32,
    ) -> Result<u32, ServiceError> {
        let ratelimit_request = self
            .build_request(ctx, scope, descriptors, hits_addend)
            .map_err(|e| ServiceError::DispatchFailed(format!("Failed to build request: {e}")))?;
        let outgoing_message = ratelimit_request.encode_to_vec();

        self.dispatch(
            ctx,
            &self.upstream_name,
            &self.service_name,
            &self.method,
            outgoing_message,
            self.timeout,
        )
    }

    pub fn build_request(
        &self,
        ctx: &mut ReqRespCtx,
        domain: &str,
        descriptors: Vec<RateLimitDescriptorData>,
        hits_addend: u32,
    ) -> Result<RateLimitRequest, AttributeError> {
        let mut pb_descriptors: Vec<RateLimitDescriptor> = descriptors
            .iter()
            .map(|desc| RateLimitDescriptor {
                entries: desc
                    .iter()
                    .map(|(k, v)| rate_limit_descriptor::Entry {
                        key: k.clone(),
                        value: v.clone(),
                    })
                    .collect(),
                limit: None,
            })
            .collect();

        let request_data = ctx.eval_request_data();
        if !request_data.is_empty() {
            let entries: Vec<_> = request_data
                .iter()
                .filter_map(|entry| match &entry.result {
                    Ok(AttributeState::Available(Value::String(value))) => {
                        let key = if entry.domain.is_empty() || entry.domain == "metrics.labels" {
                            entry.field.clone()
                        } else {
                            format!("{}.{}", entry.domain, entry.field)
                        };
                        Some(rate_limit_descriptor::Entry {
                            key,
                            value: value.to_string(),
                        })
                    }
                    _ => None,
                })
                .collect();

            if !entries.is_empty() {
                pb_descriptors.push(RateLimitDescriptor {
                    entries,
                    limit: None,
                });
            }
        }

        Ok(RateLimitRequest {
            domain: domain.to_string(),
            descriptors: pb_descriptors,
            hits_addend,
        })
    }
}
