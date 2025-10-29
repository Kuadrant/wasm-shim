use crate::v2::kuadrant::ReqRespCtx;

mod auth;
mod rate_limit;

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

    fn dispatch(&self, ctx: &mut ReqRespCtx, message: Vec<u8>) -> Result<u32, ServiceError>;
    fn parse_message(&self, message: Vec<u8>) -> Result<Self::Response, ServiceError>;
}
