use crate::configuration::{FailureMode, Service as ServiceConfig, ServiceType};
use crate::kuadrant::ReqRespCtx;
use log::debug;
use std::{rc::Rc, time::Duration};

mod auth;
pub mod rate_limit;

pub use auth::AuthService;
pub use rate_limit::RateLimitService;

#[derive(Clone)]
pub enum ServiceInstance {
    Auth(Rc<AuthService>),
    RateLimit(Rc<RateLimitService>),
    RateLimitCheck(Rc<RateLimitService>),
    RateLimitReport(Rc<RateLimitService>),
}

impl ServiceInstance {
    pub fn failure_mode(&self) -> FailureMode {
        match self {
            ServiceInstance::Auth(service) => service.failure_mode(),
            ServiceInstance::RateLimit(service) => service.failure_mode(),
            ServiceInstance::RateLimitCheck(service) => service.failure_mode(),
            ServiceInstance::RateLimitReport(service) => service.failure_mode(),
        }
    }
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

impl TryFrom<&ServiceConfig> for ServiceInstance {
    type Error = ServiceError;

    fn try_from(service: &ServiceConfig) -> Result<Self, Self::Error> {
        match service.service_type {
            ServiceType::Auth => Ok(ServiceInstance::Auth(Rc::new(AuthService::new(
                service.endpoint.clone(),
                service.timeout.0,
                service.failure_mode,
            )))),
            ServiceType::RateLimit => {
                Ok(ServiceInstance::RateLimit(Rc::new(RateLimitService::new(
                    service.endpoint.clone(),
                    service.timeout.0,
                    "envoy.service.ratelimit.v3.RateLimitService",
                    "ShouldRateLimit",
                    service.failure_mode,
                ))))
            }
            ServiceType::RateLimitCheck => Ok(ServiceInstance::RateLimitCheck(Rc::new(
                RateLimitService::new(
                    service.endpoint.clone(),
                    service.timeout.0,
                    "envoy.extensions.common.ratelimit.v3.RateLimitService",
                    "Check",
                    service.failure_mode,
                ),
            ))),
            ServiceType::RateLimitReport => Ok(ServiceInstance::RateLimitReport(Rc::new(
                RateLimitService::new(
                    service.endpoint.clone(),
                    service.timeout.0,
                    "envoy.extensions.common.ratelimit.v3.RateLimitService",
                    "Report",
                    service.failure_mode,
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
        debug!(
            "Dispatching gRPC call to {}: {} {}",
            upstream, service, method
        );
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
