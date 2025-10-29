use std::time::Duration;

use crate::v2::kuadrant::ReqRespCtx;

mod auth;
mod rate_limit;

pub use auth::AuthService;

#[derive(Debug)]
pub enum ServiceError {
    DispatchFailed(String),
    DecodeFailed(String),
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceError::DispatchFailed(msg) => write!(f, "Failed to dispatch gRPC call: {}", msg),
            ServiceError::DecodeFailed(msg) => write!(f, "Failed to decode response: {}", msg),
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
}
