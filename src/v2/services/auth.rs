use std::time::Duration;

use super::Service;
use crate::v2::kuadrant::ReqRespCtx;
use crate::v2::temp::GrpcRequest;

struct AuthService {
    upstream_name: String,
    service_name: String,
    method: String,
    timeout: Duration,
}

impl Service for AuthService {
    type Response = String;

    fn dispatch(&self, _ctx: &mut ReqRespCtx, _scope: String) -> usize {
        // build message
        // let _msg = self.request_message(ctx);

        // send message

        todo!()
    }

    fn parse_message(&self, _message: Vec<u8>) -> Self::Response {
        todo!()
    }

    fn request_message(&self, _ctx: &mut ReqRespCtx, _scope: String) -> GrpcRequest {
        todo!()
    }
}
