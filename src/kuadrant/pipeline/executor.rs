use tracing::error;

use crate::kuadrant::{
    pipeline::tasks::{
        noop_response_processor, PendingTask, Task, TaskOutcome, TeardownAction, TeardownOutcome,
    },
    ReqRespCtx,
};
use std::collections::{BTreeMap, HashSet};

pub struct Pipeline {
    pub ctx: ReqRespCtx,
    task_queue: Vec<Box<dyn Task>>,
    deferred_tasks: BTreeMap<u32, Box<dyn Task>>,
    completed_tasks: HashSet<String>,
    teardown_tasks: Vec<Box<dyn TeardownAction>>,
}

impl Pipeline {
    pub fn new(ctx: ReqRespCtx) -> Self {
        Self {
            ctx,
            task_queue: Vec::new(),
            deferred_tasks: BTreeMap::new(),
            completed_tasks: HashSet::new(),
            teardown_tasks: Vec::new(),
        }
    }

    pub fn with_tasks(mut self, tasks: Vec<Box<dyn Task>>) -> Self {
        self.task_queue = tasks;
        self
    }

    pub fn with_teardown_tasks(mut self, tasks: Vec<Box<dyn TeardownAction>>) -> Self {
        self.teardown_tasks = tasks;
        self
    }

    fn replace_deferred_with_noop(&mut self) {
        // map existing deferred tasks to no-op consumers
        let deferred = std::mem::take(&mut self.deferred_tasks);
        self.deferred_tasks = deferred
            .into_iter()
            .map(|(token_id, task)| {
                // Create a new PendingTask with no-op processor
                let pending = Box::new(PendingTask::new(
                    task.id().unwrap_or_default(),
                    Box::new(noop_response_processor(token_id)),
                )) as Box<dyn Task>;
                (token_id, pending)
            })
            .collect();
    }

    fn execute_teardown(&mut self) {
        for action in self.teardown_tasks.drain(..) {
            match action.execute(&mut self.ctx) {
                TeardownOutcome::Done => {}
                TeardownOutcome::Deferred(token_id) => {
                    // Create a no-op PendingTask for this deferred teardown action
                    let pending = Box::new(PendingTask::new(
                        format!("teardown_{}", token_id),
                        Box::new(noop_response_processor(token_id)),
                    ));
                    if self.deferred_tasks.insert(token_id, pending).is_some() {
                        error!("Duplicate token_id during teardown: {}", token_id);
                    }
                }
            }
        }
    }

    pub fn eval(mut self) -> Option<Self> {
        let tasks_to_process: Vec<_> = self.task_queue.drain(..).collect();

        for task in tasks_to_process {
            if task
                .dependencies()
                .iter()
                .any(|dep| !self.completed_tasks.contains(dep))
            {
                self.task_queue.push(task);
                continue;
            }

            let task_id = task.id();
            match task.apply(&mut self.ctx) {
                TaskOutcome::Done => {
                    if let Some(id) = task_id {
                        self.completed_tasks.insert(id);
                    }
                }
                TaskOutcome::Deferred { token_id, pending } => {
                    if self.deferred_tasks.insert(token_id, pending).is_some() {
                        error!("Duplicate token_id: {}", token_id);
                    }
                }
                TaskOutcome::Requeued(tasks) => {
                    self.task_queue.extend(tasks);
                }
                TaskOutcome::Failed => {
                    // todo(refactor): error handling
                    error!("Task failed: {:?}", task_id);
                }
                TaskOutcome::Terminate(terminal_task) => {
                    terminal_task.apply(&mut self.ctx);
                    self.task_queue.clear();
                    self.execute_teardown();
                    return if self.deferred_tasks.is_empty() {
                        None
                    } else {
                        Some(self)
                    };
                }
            }
        }

        if self.task_queue.is_empty() && self.deferred_tasks.is_empty() {
            if !self.teardown_tasks.is_empty() {
                self.execute_teardown();
            }
            if self.deferred_tasks.is_empty() {
                None
            } else {
                Some(self)
            }
        } else {
            Some(self)
        }
    }

    pub fn digest(mut self, token_id: u32, status_code: u32, response_size: usize) -> Option<Self> {
        if let Some(pending) = self.deferred_tasks.remove(&token_id) {
            match self.ctx.set_grpc_response_data(status_code, response_size) {
                Ok(_) => {}
                Err(err) => error!("Failed to set gRPC response data: {}", err),
            };
            let task_id = pending.id();
            match pending.apply(&mut self.ctx) {
                TaskOutcome::Done => {
                    if let Some(id) = task_id {
                        self.completed_tasks.insert(id);
                    }
                }
                TaskOutcome::Requeued(tasks) => {
                    if let Some(id) = task_id {
                        self.completed_tasks.insert(id);
                    }
                    for task in tasks.into_iter().rev() {
                        self.task_queue.insert(0, task);
                    }
                }
                TaskOutcome::Deferred { token_id, pending } => {
                    if self.deferred_tasks.insert(token_id, pending).is_some() {
                        error!("Duplicate token_id: {}", token_id);
                    }
                }
                TaskOutcome::Failed => {
                    // todo(refactor): error handling
                    error!("Failed to process response for token_id: {}", token_id);
                }
                TaskOutcome::Terminate(terminal_task) => {
                    terminal_task.apply(&mut self.ctx);
                    self.task_queue.clear();
                    self.replace_deferred_with_noop();
                    self.execute_teardown();

                    return self.eval();
                }
            }
        } else {
            error!("token_id: {} not found", token_id);
        }

        self.eval()
    }

    pub fn requires_pause(&self) -> bool {
        let has_deferred = !self.deferred_tasks.is_empty();
        let has_blocking = self.task_queue.iter().any(|task| {
            // Only consider tasks whose dependencies are met.
            // Tasks with unmet deps shouldn't block the filter since they're
            // waiting for other tasks to complete first.
            let deps_met = task
                .dependencies()
                .iter()
                .all(|dep| self.completed_tasks.contains(dep));
            deps_met && task.pauses_filter()
        });

        has_deferred || has_blocking
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kuadrant::MockWasmHost;
    use std::sync::Arc;

    fn create_test_context() -> ReqRespCtx {
        let mock_host = MockWasmHost::new();
        ReqRespCtx::new(Arc::new(mock_host))
    }

    /// A simple test task for testing requires_pause behavior
    struct TestTask {
        id: String,
        dependencies: Vec<String>,
        pauses_filter: bool,
    }

    impl TestTask {
        fn new(id: &str, dependencies: Vec<&str>, pauses_filter: bool) -> Self {
            Self {
                id: id.to_string(),
                dependencies: dependencies.into_iter().map(|s| s.to_string()).collect(),
                pauses_filter,
            }
        }
    }

    impl Task for TestTask {
        fn apply(self: Box<Self>, _ctx: &mut ReqRespCtx) -> TaskOutcome {
            TaskOutcome::Done
        }

        fn id(&self) -> Option<String> {
            Some(self.id.clone())
        }

        fn dependencies(&self) -> &[String] {
            &self.dependencies
        }

        fn pauses_filter(&self) -> bool {
            self.pauses_filter
        }
    }

    #[test]
    fn requires_pause_ignores_tasks_with_unmet_deps() {
        // Task with pauses_filter=true but depends on "auth" which hasn't completed
        let blocking_task = TestTask::new("blocker", vec!["auth"], true);

        let ctx = create_test_context();
        let mut pipeline = Pipeline::new(ctx);
        pipeline.task_queue.push(Box::new(blocking_task));

        // requires_pause should return FALSE because the task's deps are not met
        assert!(
            !pipeline.requires_pause(),
            "requires_pause() should ignore tasks with unmet deps"
        );
    }

    #[test]
    fn requires_pause_considers_tasks_with_met_deps() {
        // Task with pauses_filter=true and deps are met (auth is in completed_tasks)
        let blocking_task = TestTask::new("blocker", vec!["auth"], true);

        let ctx = create_test_context();
        let mut pipeline = Pipeline::new(ctx);
        pipeline.completed_tasks.insert("auth".to_string());
        pipeline.task_queue.push(Box::new(blocking_task));

        // requires_pause should return TRUE because task's deps are met and it pauses
        assert!(
            pipeline.requires_pause(),
            "requires_pause() should consider tasks with met deps"
        );
    }

    #[test]
    fn requires_pause_returns_true_for_deferred_tasks() {
        // Even with blocking task that has unmet deps, deferred tasks cause pause
        let blocking_task = TestTask::new("blocker", vec!["auth"], true);
        let deferred_task = TestTask::new("auth", vec![], false);

        let ctx = create_test_context();
        let mut pipeline = Pipeline::new(ctx);
        pipeline.task_queue.push(Box::new(blocking_task));
        pipeline.deferred_tasks.insert(42, Box::new(deferred_task));

        // requires_pause should return TRUE because there's a deferred task
        assert!(
            pipeline.requires_pause(),
            "requires_pause() should return true when deferred tasks exist"
        );
    }

    #[test]
    fn requires_pause_ignores_non_blocking_tasks_with_met_deps() {
        // Task with pauses_filter=false and deps met - should not cause pause
        let non_blocking_task = TestTask::new("report", vec!["auth"], false);

        let ctx = create_test_context();
        let mut pipeline = Pipeline::new(ctx);
        pipeline.completed_tasks.insert("auth".to_string());
        pipeline.task_queue.push(Box::new(non_blocking_task));

        // requires_pause should return FALSE because task doesn't pause filter
        assert!(
            !pipeline.requires_pause(),
            "requires_pause() should ignore tasks with pauses_filter=false"
        );
    }
}
