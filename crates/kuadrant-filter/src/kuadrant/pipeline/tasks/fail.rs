use tracing::error;

use crate::data::attribute::AttributeState;
use crate::data::cel::Predicate;
use crate::kuadrant::pipeline::tasks::{SendReplyTask, Task, TaskOutcome};
use crate::kuadrant::ReqRespCtx;
pub struct FailTask {
    task_id: String,
    predicate: Predicate,
    log_message: String,
    terminal: bool,
}

impl FailTask {
    pub fn new(task_id: String, predicate: Predicate, log_message: String, terminal: bool) -> Self {
        Self {
            task_id,
            predicate,
            log_message,
            terminal,
        }
    }
}

impl Task for FailTask {
    fn id(&self) -> &str {
        &self.task_id
    }

    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        let mut cel_ctx = ctx.cel.new_ctx(&*self);
        match self.predicate.test(ctx, &mut cel_ctx) {
            Ok(AttributeState::Available(true)) => {
                error!("Action failure: {}", self.log_message);
                if self.terminal {
                    ctx.metrics().increment_errors();
                    TaskOutcome::Terminate(Box::new(SendReplyTask::default()))
                } else {
                    TaskOutcome::Done
                }
            }
            Ok(AttributeState::Available(false)) => TaskOutcome::Done,
            Ok(AttributeState::Pending) => {
                if (self.predicate.has_request_body_deps() && ctx.request_body.is_end_of_stream())
                    || (self.predicate.has_response_body_deps()
                        && ctx.response_body.is_end_of_stream())
                {
                    TaskOutcome::Failed
                } else {
                    TaskOutcome::Requeued(vec![self])
                }
            }
            Err(e) => {
                error!("Failed to evaluate log task predicate: {e:?}");
                TaskOutcome::Failed
            }
        }
    }
}
