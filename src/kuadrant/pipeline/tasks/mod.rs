mod auth;
mod export_traces;
mod failure_mode;
mod headers;
mod ratelimit;
mod send_reply;
mod store_data;
mod token_usage;

pub use auth::AuthTask;
pub use export_traces::ExportTracesTask;
pub use failure_mode::FailureModeTask;
pub use headers::{HeaderOperation, HeadersType, ModifyHeadersTask};
pub use ratelimit::RateLimitTask;
pub use send_reply::SendReplyTask;
pub use store_data::StoreDataTask;
pub use token_usage::TokenUsageTask;

use crate::kuadrant::ReqRespCtx;

//todo(refactor): this now has the signature of a task; should it be one?
pub type ResponseProcessor = dyn FnOnce(&mut ReqRespCtx) -> TaskOutcome;

pub trait Task {
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome;

    fn id(&self) -> Option<String> {
        None
    }

    fn dependencies(&self) -> &[String] {
        &[]
    }

    fn pauses_filter(&self) -> bool {
        false
    }
}

pub struct PendingTask {
    task_id: String,
    process_response: Box<ResponseProcessor>,
}

impl Task for PendingTask {
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        (self.process_response)(ctx)
    }
    fn id(&self) -> Option<String> {
        Some(self.task_id.clone())
    }
    fn pauses_filter(&self) -> bool {
        true
    }
}

pub enum TaskOutcome {
    Done,
    Deferred {
        token_id: u32,
        pending: Box<dyn Task>,
    },
    Requeued(Vec<Box<dyn Task>>),
    Failed,
    Terminate(Box<dyn Task>),
}
