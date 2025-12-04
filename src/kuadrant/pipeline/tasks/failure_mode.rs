use crate::kuadrant::{
    pipeline::tasks::{SendReplyTask, Task, TaskOutcome},
    ReqRespCtx,
};
use crate::metrics::METRICS;

pub struct FailureModeTask {
    task: Box<dyn Task>,
    abort: bool,
}

impl FailureModeTask {
    pub fn new(task: Box<dyn Task>, abort: bool) -> Self {
        Self { task, abort }
    }
}

impl Task for FailureModeTask {
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        match self.task.apply(ctx) {
            TaskOutcome::Failed => {
                METRICS.errors().increment();
                if self.abort {
                    let span = tracing::Span::current();
                    span.record("otel.status_code", "ERROR");
                    TaskOutcome::Terminate(Box::new(SendReplyTask::default()))
                } else {
                    TaskOutcome::Done
                }
            }
            TaskOutcome::Deferred { token_id, pending } => TaskOutcome::Deferred {
                token_id,
                pending: Box::new(FailureModeTask {
                    task: pending,
                    abort: self.abort,
                }),
            },
            outcome => outcome,
        }
    }

    fn id(&self) -> Option<String> {
        self.task.id()
    }

    fn dependencies(&self) -> &[String] {
        self.task.dependencies()
    }

    fn pauses_filter(&self) -> bool {
        self.task.pauses_filter()
    }
}
