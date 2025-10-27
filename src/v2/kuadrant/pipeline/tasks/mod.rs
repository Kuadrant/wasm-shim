#[allow(dead_code)]
mod headers;
mod ratelimit;
mod sse_parser;

use crate::v2::kuadrant::ReqRespCtx;
use crate::v2::services::Service;
use std::collections::HashSet;
use std::rc::Rc;

#[allow(dead_code)]
pub trait Task {
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome;

    fn id(&self) -> Option<String> {
        None
    }

    fn dependencies_met(&self, _completed_tasks: &HashSet<String>) -> bool {
        true
    }
}

#[allow(dead_code)]
pub struct PendingTask {
    task_id: Option<String>,
    is_blocking: bool,
    allow_task: Option<Box<dyn Task>>,
    deny_task: Box<dyn Task>,
    service: Rc<dyn Service<Response = bool>>,
}

#[allow(dead_code)]
impl PendingTask {
    pub fn task_id(&self) -> Option<&String> {
        self.task_id.as_ref()
    }

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
    Continue(Box<dyn Task>),
    Deferred {
        token_id: usize,
        pending: PendingTask,
    },
    Requeued(Box<dyn Task>),
    Failed, // Possibly wrapping an error
}
