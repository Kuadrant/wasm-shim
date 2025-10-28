#[allow(dead_code)]
mod headers;
mod ratelimit;
mod send;
mod token_usage;

use crate::v2::kuadrant::ReqRespCtx;

pub type ResponseProcessor<T> = dyn FnOnce(T) -> Vec<Box<dyn Task>>;

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
    process_response: Box<ResponseProcessor<Vec<u8>>>,
}

#[allow(dead_code)]
impl PendingTask {
    pub fn task_id(&self) -> Option<&String> {
        self.task_id.as_ref()
    }

    pub fn process_response(self, response: Vec<u8>) -> Vec<Box<dyn Task>> {
        (self.process_response)(response)
    }

    pub fn is_blocking(&self) -> bool {
        // This would need to peak into `ok_action` AND `rl_action` to see if we need to block
        self.is_blocking
    }
}

#[allow(dead_code)]
pub enum TaskOutcome {
    Done,
    Continue(Box<dyn Task>),
    Deferred {
        token_id: usize,
        pending: PendingTask,
    },
    Requeued(Box<dyn Task>),
    Failed, // Possibly wrapping an error
}
