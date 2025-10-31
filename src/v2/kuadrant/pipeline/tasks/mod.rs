mod auth;
mod headers;
mod ratelimit;
mod send_reply;
mod store_data;
mod token_usage;

pub use auth::AuthTask;
pub use headers::{HeaderOperation, HeadersType, ModifyHeadersTask};
pub use send_reply::SendReplyTask;
pub use store_data::StoreDataTask;

use crate::v2::kuadrant::ReqRespCtx;

pub type ResponseProcessor = dyn FnOnce(&mut ReqRespCtx, u32, usize) -> TaskOutcome;

#[allow(dead_code)]
pub trait Task {
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome;

    fn id(&self) -> Option<String> {
        None
    }

    fn dependencies(&self) -> &[String] {
        &[]
    }
}

#[allow(dead_code)]
pub struct PendingTask {
    task_id: Option<String>,
    is_blocking: bool,
    process_response: Box<ResponseProcessor>,
}

#[allow(dead_code)]
impl PendingTask {
    pub fn task_id(&self) -> Option<&String> {
        self.task_id.as_ref()
    }

    pub fn process_response(
        self,
        ctx: &mut ReqRespCtx,
        status_code: u32,
        response_size: usize,
    ) -> TaskOutcome {
        (self.process_response)(ctx, status_code, response_size)
    }

    pub fn is_blocking(&self) -> bool {
        // This would need to peak into `ok_action` AND `rl_action` to see if we need to block
        self.is_blocking
    }
}

#[allow(dead_code)]
pub enum TaskOutcome {
    Done,
    Deferred { token_id: u32, pending: PendingTask },
    Requeued(Vec<Box<dyn Task>>),
    Failed,
    Terminate(Box<dyn Task>),
}
