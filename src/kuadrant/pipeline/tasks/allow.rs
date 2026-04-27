use crate::data::attribute::AttributeState;
use crate::data::cel::{Predicate, PredicateVec};
use crate::kuadrant::pipeline::tasks::{SendReplyTask, Task, TaskOutcome};
use crate::kuadrant::ReqRespCtx;
use tracing::error;

pub struct AllowTask {
    task_id: String,
    predicates: Vec<Predicate>,
    intention: Predicate,
    dependencies: Vec<String>,
}

impl AllowTask {
    pub fn new(
        task_id: String,
        predicates: Vec<Predicate>,
        intention: Predicate,
        dependencies: Vec<String>,
    ) -> Self {
        Self {
            task_id,
            predicates,
            intention,
            dependencies,
        }
    }
}

impl Task for AllowTask {
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
                error!("AllowTask predicates failed: {e:?}");
                return TaskOutcome::Failed;
            }
        }

        match self.intention.test(ctx) {
            Ok(AttributeState::Pending) => TaskOutcome::Requeued(vec![self]),
            Ok(AttributeState::Available(true)) => TaskOutcome::Done,
            Ok(AttributeState::Available(false)) => {
                TaskOutcome::Terminate(Box::new(SendReplyTask::new(403, vec![], None)))
            }
            Err(e) => {
                error!("AllowTask intention evaluation failed: {e:?}");
                TaskOutcome::Failed
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kuadrant::resolver::MockWasmHost;
    use std::sync::Arc;

    #[test]
    fn allow_task_intention_true_returns_done() {
        let mock_host = MockWasmHost::new();
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));

        let task = Box::new(AllowTask::new(
            "test".to_string(),
            vec![],
            Predicate::new("true").unwrap(),
            vec![],
        ));

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Done));
    }

    #[test]
    fn allow_task_intention_false_terminates() {
        let mock_host = MockWasmHost::new();
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));

        let task = Box::new(AllowTask::new(
            "test".to_string(),
            vec![],
            Predicate::new("false").unwrap(),
            vec![],
        ));

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Terminate(_)));
    }

    #[test]
    fn allow_task_predicate_false_skips() {
        let mock_host = MockWasmHost::new();
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));

        let task = Box::new(AllowTask::new(
            "test".to_string(),
            vec![Predicate::new("false").unwrap()],
            Predicate::new("false").unwrap(), // would deny, but predicates skip first
            vec![],
        ));

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Done));
    }
}
