use crate::data::attribute::AttributeState;
use crate::data::cel::{Predicate, PredicateVec};
use crate::kuadrant::pipeline::tasks::{SendReplyTask, Task, TaskOutcome};
use crate::kuadrant::ReqRespCtx;
use tracing::error;

pub struct WithResponseCodeTask {
    task_id: String,
    predicates: Vec<Predicate>,
    new_response_code: u32,
    dependencies: Vec<String>,
}

impl WithResponseCodeTask {
    pub fn new(
        task_id: String,
        predicates: Vec<Predicate>,
        new_response_code: u32,
        dependencies: Vec<String>,
    ) -> Self {
        Self {
            task_id,
            predicates,
            new_response_code,
            dependencies,
        }
    }
}

impl Task for WithResponseCodeTask {
    fn id(&self) -> Option<String> {
        Some(self.task_id.clone())
    }

    fn dependencies(&self) -> &[String] {
        &self.dependencies
    }

    fn pauses_filter(&self) -> bool {
        false
    }

    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        match self.predicates.apply(ctx) {
            Ok(AttributeState::Pending) => return TaskOutcome::Requeued(vec![self]),
            Ok(AttributeState::Available(false)) => return TaskOutcome::Done,
            Ok(AttributeState::Available(true)) => {}
            Err(e) => {
                error!("WithResponseCodeTask predicates failed: {e:?}");
                return TaskOutcome::Failed;
            }
        }

        TaskOutcome::Terminate(Box::new(SendReplyTask::new(
            self.new_response_code,
            vec![],
            None,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kuadrant::resolver::MockWasmHost;
    use std::sync::Arc;

    #[test]
    fn with_response_code_terminates_with_code() {
        let mock_host = MockWasmHost::new();
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));

        let task = Box::new(WithResponseCodeTask::new(
            "test".to_string(),
            vec![],
            403,
            vec![],
        ));

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Terminate(_)));
    }

    #[test]
    fn with_response_code_predicate_false_skips() {
        let mock_host = MockWasmHost::new();
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));

        let task = Box::new(WithResponseCodeTask::new(
            "test".to_string(),
            vec![Predicate::new("false").unwrap()],
            403,
            vec![],
        ));

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Done));
    }
}
