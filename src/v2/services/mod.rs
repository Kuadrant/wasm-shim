use std::{rc::Rc, time::Duration};

use crate::v2::configuration::ServiceType;
use crate::v2::kuadrant::ReqRespCtx;

mod auth;
mod rate_limit;

pub use auth::AuthService;
pub use rate_limit::RateLimitService;

#[derive(Clone)]
pub enum ServiceInstance {
    Auth(Rc<AuthService>),
    RateLimit(Rc<RateLimitService>),
    RateLimitCheck(Rc<RateLimitService>),
    RateLimitReport(Rc<RateLimitService>),
}

#[derive(Debug)]
pub enum ServiceError {
    Dispatch(String),
    Decode(String),
    Retrieval(String),
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceError::Dispatch(msg) => write!(f, "Failed to dispatch gRPC call: {}", msg),
            ServiceError::Decode(msg) => write!(f, "Failed to decode response: {}", msg),
            ServiceError::Retrieval(msg) => {
                write!(f, "Failed to retrieve gRPC response: {}", msg)
            }
        }
    }
}

impl std::error::Error for ServiceError {}

impl TryFrom<(String, Duration, ServiceType)> for ServiceInstance {
    type Error = ServiceError;

    fn try_from(value: (String, Duration, ServiceType)) -> Result<Self, Self::Error> {
        let (endpoint, timeout, service_type) = value;

        match service_type {
            ServiceType::Auth => Ok(ServiceInstance::Auth(Rc::new(AuthService::new(
                endpoint, timeout,
            )))),
            ServiceType::RateLimit => {
                Ok(ServiceInstance::RateLimit(Rc::new(RateLimitService::new(
                    endpoint,
                    timeout,
                    "envoy.service.ratelimit.v3.RateLimitService",
                    "ShouldRateLimit",
                ))))
            }
            ServiceType::RateLimitCheck => Ok(ServiceInstance::RateLimitCheck(Rc::new(
                RateLimitService::new(
                    endpoint,
                    timeout,
                    "envoy.extensions.common.ratelimit.v3.RateLimitService",
                    "Check",
                ),
            ))),
            ServiceType::RateLimitReport => Ok(ServiceInstance::RateLimitReport(Rc::new(
                RateLimitService::new(
                    endpoint,
                    timeout,
                    "envoy.extensions.common.ratelimit.v3.RateLimitService",
                    "Report",
                ),
            ))),
        }
    }
}

pub trait Service {
    type Response;

    fn dispatch(
        &self,
        ctx: &mut ReqRespCtx,
        upstream: &str,
        service: &str,
        method: &str,
        message: Vec<u8>,
        timeout: Duration,
    ) -> Result<u32, ServiceError> {
        ctx.dispatch_grpc_call(upstream, service, method, message, timeout)
    }

    fn parse_message(&self, message: Vec<u8>) -> Result<Self::Response, ServiceError>;

    fn get_response(
        &self,
        ctx: &mut ReqRespCtx,
        response_size: usize,
    ) -> Result<Self::Response, ServiceError> {
        let message = ctx.get_grpc_response(response_size)?;
        self.parse_message(message)
    }
}
