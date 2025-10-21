mod common;
mod ratelimit;

use crate::v2::kuadrant::ReqRespCtx;
use crate::v2::services::Service;
use std::rc::Rc;
trait Task {
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome;
}

struct PendingTask {
    is_blocking: bool,
    allow_task: Option<Box<dyn Task>>,
    deny_task: Box<dyn Task>,
    service: Rc<dyn Service<Response = bool>>,
}
impl PendingTask {
    fn process_response(self, response: Vec<u8>) -> Option<Box<dyn Task>> {
        if self.service.parse_message(response) {
            Some(self.deny_task)
        } else if let Some(action) = self.allow_task {
            Some(action)
        } else {
            None
        }
    }

    fn is_blocking(&self) -> bool {
        // This would need to peak into `ok_action` AND `rl_action` to see if we need to block
        self.is_blocking
    }
}

enum TaskOutcome {
    Done,
    Deferred((usize, PendingTask)),
    Pending(Box<dyn Task>),
    Failed, // Possibly wrapping an error
}
