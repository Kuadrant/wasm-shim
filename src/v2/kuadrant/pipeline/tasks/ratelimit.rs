#[allow(dead_code)]
use crate::v2::data::cel::Predicate;
use crate::v2::kuadrant::pipeline::tasks::{Task, TaskOutcome};
use crate::v2::kuadrant::ReqRespCtx;
use crate::v2::services::Service;
use std::rc::Rc;

#[allow(dead_code)]
struct RateLimitTask {
    scope: String,
    predicate: Predicate,
    service: Rc<dyn Service<Response = bool>>,
    allow_task: Option<Box<dyn Task>>,
    deny_task: Box<dyn Task>,
}

impl Task for RateLimitTask {
    fn apply(self: Box<Self>, _: &mut ReqRespCtx) -> TaskOutcome {
        // match self.predicate.eval(ctx) taht returns Result<AttributeState<Value>, CelError>
        // if AttributeState(ok) --> self.service.dispatch, TaskOutcome::Deferred
        // else TaskOutcome::Done
        // if err (?) TaskOutcome::Failed(self) ?

        TaskOutcome::Done
    }
}

#[allow(dead_code)]
struct TooManyRequestsTask {}

impl Task for TooManyRequestsTask {
    fn apply(self: Box<Self>, _: &mut ReqRespCtx) -> TaskOutcome {
        // ctx.send_message 429
        TaskOutcome::Done
    }
}
