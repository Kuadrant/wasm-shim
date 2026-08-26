use crate::configuration::{FailureMode, Service as ServiceConfig, ServiceType};
use crate::filter::DescriptorManager;
use crate::kuadrant::ReqRespCtx;
use std::{sync::Arc, time::Duration};

mod dynamic;
mod tracing;

pub use dynamic::converters::{
    cel_value_to_header_pairs, deny_response_struct_def, DescriptorConverter, MessageConverter,
};
pub use dynamic::DynamicService;
pub use tracing::TracingService;

#[derive(Clone)]
pub enum ServiceInstance {
    Tracing(Option<Arc<TracingService>>),
    Dynamic(Arc<DynamicService>),
}

impl ServiceInstance {
    pub fn failure_mode(&self) -> FailureMode {
        match self {
            ServiceInstance::Dynamic(service) => service.failure_mode(),
            ServiceInstance::Tracing(_) => FailureMode::Allow,
        }
    }

    pub fn from_config(
        service: ServiceConfig,
        descriptor_manager: &Arc<DescriptorManager>,
    ) -> Result<Self, ServiceError> {
        match service.service_type {
            ServiceType::Tracing => Ok(ServiceInstance::Tracing(Some(Arc::new(
                TracingService::new(service.endpoint, service.timeout.0),
            )))),
            ServiceType::Dynamic => {
                let grpc_service = service.grpc_service.as_ref().ok_or_else(|| {
                    ServiceError::Dispatch("Missing grpc_service for Dynamic service".to_string())
                })?;
                let grpc_method = service.grpc_method.as_ref().ok_or_else(|| {
                    ServiceError::Dispatch("Missing grpc_method for Dynamic service".to_string())
                })?;

                Ok(ServiceInstance::Dynamic(Arc::new(DynamicService::new(
                    service.endpoint,
                    grpc_service.clone(),
                    grpc_method.clone(),
                    service.timeout.0,
                    service.failure_mode,
                    Arc::clone(descriptor_manager),
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
