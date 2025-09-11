use std::time::Duration;

use crate::v2::kuadrant::ReqRespCtx;
use crate::v2::kuadrant::Service;
use crate::v2::temp::GrpcRequest;

struct AuthService {
    upstream_name: String,
    service_name: String,
    method: String,
    timeout: Duration,
}

impl Service for AuthService {
    type Response = String;

    fn dispatch(&self, ctx: &mut ReqRespCtx) -> usize {
        // build message
        let _msg = self.request_message(ctx);

        // send message

        todo!()
    }

    fn parse_message(&self, message: Vec<u8>) -> Self::Response {
        todo!()
    }

    fn request_message(&self, ctx: &mut ReqRespCtx) -> GrpcRequest {
        ctx.get_attribute::<String>("request.path".into());
        todo!()
    }
}
