use crate::v2::kuadrant::ReqRespCtx;
use crate::v2::temp::GrpcRequest;

pub mod auth;

pub trait Service {
    type Response;
    fn dispatch(&self, ctx: &mut ReqRespCtx, scope: String) -> usize;
    fn parse_message(&self, message: Vec<u8>) -> Self::Response;

    #[deprecated]
    fn request_message(&self, ctx: &mut ReqRespCtx, scope: String) -> GrpcRequest;
}
