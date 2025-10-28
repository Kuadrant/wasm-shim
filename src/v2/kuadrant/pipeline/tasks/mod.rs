#[allow(dead_code)]
mod headers;
mod ratelimit;

use crate::v2::kuadrant::ReqRespCtx;
use crate::v2::services::Service;
use std::rc::Rc;

#[allow(dead_code)]
pub trait Task {
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome;
}

#[allow(dead_code)]
pub struct PendingTask {
    is_blocking: bool,
    allow_task: Option<Box<dyn Task>>,
    deny_task: Box<dyn Task>,
    service: Rc<dyn Service<Response = bool>>,
}

#[allow(dead_code)]
impl PendingTask {
    pub fn process_response(self, response: Vec<u8>) -> Option<Box<dyn Task>> {
        if self.service.parse_message(response) {
            Some(self.deny_task)
        } else if let Some(action) = self.allow_task {
            Some(action)
        } else {
            None
        }
    }

    pub fn is_blocking(&self) -> bool {
        // This would need to peak into `ok_action` AND `rl_action` to see if we need to block
        self.is_blocking
    }
}

#[allow(dead_code)]
pub enum TaskOutcome {
    Done,
    Deferred {
        token_id: usize,
        pending: PendingTask,
    },
    Requeued(Box<dyn Task>),
    Failed, // Possibly wrapping an error
}
