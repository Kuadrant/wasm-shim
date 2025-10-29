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

    fn dispatch(&self, ctx: &mut ReqRespCtx, message: Vec<u8>) -> Result<u32, ServiceError> {
        ctx.dispatch_grpc_call(
            &self.upstream_name,
            &self.service_name,
            &self.method,
            message,
            self.timeout,
        )
    }

    fn parse_message(&self, message: Vec<u8>) -> Result<Self::Response, ServiceError> {
        prost::Message::decode(&message[..])
            .map_err(|e| ServiceError::DecodeFailed(format!("RateLimitResponse: {e}")))
    }
}

impl RateLimitService {
    pub fn build_request(
        &self,
        ctx: &mut ReqRespCtx,
        domain: String,
        descriptors: Vec<RateLimitDescriptorData>,
        hits_addend: u32,
    ) -> Result<Vec<u8>, AttributeError> {
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

        let request = RateLimitRequest {
            domain,
            descriptors: pb_descriptors,
            hits_addend,
        };

        Ok(request.encode_to_vec())
    }
}
