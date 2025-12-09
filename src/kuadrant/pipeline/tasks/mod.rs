mod auth;
mod export_traces;
mod failure_mode;
mod headers;
mod ratelimit;
mod send_reply;
mod store_data;
mod token_usage;
mod tracing_decorator;

pub use auth::AuthTask;
pub use export_traces::ExportTracesTask;
pub use failure_mode::FailureModeTask;
pub use headers::{HeaderOperation, HeadersType, ModifyHeadersTask};
pub use ratelimit::RateLimitTask;
pub use send_reply::SendReplyTask;
pub use store_data::StoreDataTask;
pub use token_usage::TokenUsageTask;
use tracing::debug;
pub use tracing_decorator::TracingDecoratorTask;

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

impl PendingTask {
    pub fn new(task_id: String, process_response: Box<ResponseProcessor>) -> Self {
        Self {
            task_id,
            process_response,
        }
    }
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

pub trait TeardownAction {
    fn execute(self: Box<Self>, ctx: &mut ReqRespCtx) -> TeardownOutcome;
}

pub enum TeardownOutcome {
    Done,
    Deferred(u32),
}

pub fn noop_response_processor(token_id: u32) -> impl FnOnce(&mut ReqRespCtx) -> TaskOutcome {
    move |ctx: &mut ReqRespCtx| {
        match ctx.get_grpc_response_data() {
            Ok((status_code, _response_size)) => {
                if status_code != 0 {
                    debug!(
                        "gRPC request failed with status {} (token_id: {})",
                        status_code, token_id
                    );
                }
            }
            Err(e) => {
                debug!(
                    "Failed to get gRPC response for token_id {}: {:?}",
                    token_id, e
                );
            }
        }
        TaskOutcome::Done
    }
}
