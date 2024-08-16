pub(crate) mod auth;
pub(crate) mod rate_limit;

use protobuf::Message;
use proxy_wasm::types::Status;

pub trait Service<M: Message> {
    fn send(&self, message: M) -> Result<u32, Status>;
}
