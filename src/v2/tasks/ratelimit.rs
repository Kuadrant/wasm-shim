use crate::v2::data::cel::Predicate;
use crate::v2::kuadrant::ReqRespCtx;
use crate::v2::services::Service;
use crate::v2::tasks::{Task, TaskOutcome};
use std::rc::Rc;

struct RateLimitTask {
    scope: String,
    predicate: Predicate,
    service: Rc<dyn Service<Response = bool>>,
    allow_task: Option<Box<dyn Task>>,
    deny_task: Box<dyn Task>,
}

impl Task for RateLimitTask {
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        // match self.predicate.eval(ctx) taht returns Result<AttributeState<Value>, CelError>
        // if AttributeState(ok) --> self.service.dispatch, TaskOutcome::Deferred
        // else TaskOutcome::Done
        // if err (?) TaskOutcome::Pending(self) ?

        TaskOutcome::Done
    }
}

struct TooManyRequestsTask {}

impl Task for TooManyRequestsTask {
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        // ctx.send_message 429
        TaskOutcome::Done
    }
}
