use crate::v2::kuadrant::ReqRespCtx;
use crate::v2::tasks::{Task, TaskOutcome};

#[derive(Clone)]
struct AddResponseHeadersTask {
    headers: Vec<(String, String)>,
}

impl Task for AddResponseHeadersTask {
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        // ctx.add_headers
        // if ok --> TaskOutcome::Done
        // if err, wrong phase --> TaskOutcome::Pending(self)
        TaskOutcome::Done
    }
}
