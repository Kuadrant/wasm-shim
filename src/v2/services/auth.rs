use std::time::Duration;

use super::Service;
use crate::v2::kuadrant::ReqRespCtx;
use crate::v2::services::ServiceError;

struct AuthService {
    upstream_name: String,
    service_name: String,
    method: String,
    timeout: Duration,
}

impl Service for AuthService {
    type Response = String;

    fn dispatch(&self, _ctx: &mut ReqRespCtx, _message: Vec<u8>) -> Result<u32, ServiceError> {
        // build message
        // let _msg = self.request_message(ctx);

        // send message

        todo!()
    }

    fn parse_message(&self, _message: Vec<u8>) -> Result<Self::Response, ServiceError> {
        todo!()
    }
}
