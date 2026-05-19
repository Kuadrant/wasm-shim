use crate::configuration::{FailureMode, Service as ServiceConfig, ServiceType};
use crate::filter::DescriptorManager;
use crate::kuadrant::ReqRespCtx;
use std::{rc::Rc, time::Duration};

mod auth;
mod dynamic;
mod tracing;

pub use auth::AuthService;
pub use dynamic::converters::{cel_value_to_header_pairs, ConversionError, MessageConverter};
pub use dynamic::DynamicService;
pub use tracing::TracingService;

#[derive(Clone)]
pub enum ServiceInstance {
    Auth(Rc<AuthService>),
    RateLimit(Rc<DynamicService>),
    RateLimitCheck(Rc<DynamicService>),
    RateLimitReport(Rc<DynamicService>),
    Tracing(Option<Rc<TracingService>>),
    Dynamic(Rc<DynamicService>),
}

impl ServiceInstance {
    pub fn failure_mode(&self) -> FailureMode {
        match self {
            ServiceInstance::Auth(service) => service.failure_mode(),
            ServiceInstance::RateLimit(service) => service.failure_mode(),
            ServiceInstance::RateLimitCheck(service) => service.failure_mode(),
            ServiceInstance::RateLimitReport(service) => service.failure_mode(),
            ServiceInstance::Tracing(_) => FailureMode::Allow,
            ServiceInstance::Dynamic(service) => service.failure_mode(),
        }
    }

    pub fn from_config(
        service: ServiceConfig,
        descriptor_manager: &Rc<DescriptorManager>,
    ) -> Result<Self, ServiceError> {
        match service.service_type {
            ServiceType::Auth => Ok(ServiceInstance::Auth(Rc::new(AuthService::new(
                service.endpoint,
                service.timeout.0,
                service.failure_mode,
            )))),
            ServiceType::RateLimit => Ok(ServiceInstance::RateLimit(Rc::new(DynamicService::new(
                service.endpoint,
                "envoy.service.ratelimit.v3.RateLimitService".to_string(),
                "ShouldRateLimit".to_string(),
                service.timeout.0,
                service.failure_mode,
                Rc::clone(descriptor_manager),
            )))),
            ServiceType::RateLimitCheck => Ok(ServiceInstance::RateLimitCheck(Rc::new(
                DynamicService::new(
                    service.endpoint,
                    "kuadrant.service.ratelimit.v1.RateLimitService".to_string(),
                    "CheckRateLimit".to_string(),
                    service.timeout.0,
                    service.failure_mode,
                    Rc::clone(descriptor_manager),
                ),
            ))),
            ServiceType::RateLimitReport => Ok(ServiceInstance::RateLimitReport(Rc::new(
                DynamicService::new(
                    service.endpoint,
                    "kuadrant.service.ratelimit.v1.RateLimitService".to_string(),
                    "Report".to_string(),
                    service.timeout.0,
                    service.failure_mode,
                    Rc::clone(descriptor_manager),
                ),
            ))),
            ServiceType::Tracing => Ok(ServiceInstance::Tracing(Some(Rc::new(
                TracingService::new(service.endpoint, service.timeout.0),
            )))),
            ServiceType::Dynamic => {
                let grpc_service = service.grpc_service.as_ref().ok_or_else(|| {
                    ServiceError::Dispatch("Missing grpc_service for Dynamic service".to_string())
                })?;
                let grpc_method = service.grpc_method.as_ref().ok_or_else(|| {
                    ServiceError::Dispatch("Missing grpc_method for Dynamic service".to_string())
                })?;

                Ok(ServiceInstance::Dynamic(Rc::new(DynamicService::new(
                    service.endpoint,
                    grpc_service.clone(),
                    grpc_method.clone(),
                    service.timeout.0,
                    service.failure_mode,
                    Rc::clone(descriptor_manager),
                ))))
            }
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
