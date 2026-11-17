use tracing::error;

use crate::kuadrant::{
    pipeline::tasks::{
        noop_response_processor, PendingTask, Task, TaskOutcome, TeardownAction, TeardownOutcome,
    },
    ReqRespCtx,
};
use std::collections::{BTreeMap, HashSet};
use std::ops::Not;

pub enum PipelineState {
    InProgress(Box<Pipeline>),
    Completed { should_resume: bool },
}

enum ProcessOutcome {
    Continue,
    Terminated,
}

pub struct Pipeline {
    pub ctx: ReqRespCtx,
    task_queue: Vec<Box<dyn Task>>,
    access_log_task_queue: Vec<Box<dyn Task>>,
    deferred_tasks: BTreeMap<u32, Box<dyn Task>>,
    settled_tasks: HashSet<String>,
    teardown_tasks: Vec<Box<dyn TeardownAction>>,
    terminated: bool,
}

impl From<Pipeline> for PipelineState {
    fn from(pipeline: Pipeline) -> Self {
        if pipeline.task_queue.is_empty()
            && pipeline.access_log_task_queue.is_empty()
            && pipeline.deferred_tasks.is_empty()
        {
            PipelineState::Completed {
                should_resume: pipeline.terminated.not(),
            }
        } else {
            PipelineState::InProgress(Box::new(pipeline))
        }
    }
}

impl Pipeline {
    pub fn new(ctx: ReqRespCtx) -> Self {
        Self {
            ctx,
            task_queue: Vec::new(),
            access_log_task_queue: Vec::new(),
            deferred_tasks: BTreeMap::new(),
            settled_tasks: HashSet::new(),
            teardown_tasks: Vec::new(),
            terminated: false,
        }
    }

    pub fn with_tasks(mut self, tasks: Vec<Box<dyn Task>>) -> Self {
        self.enqueue_tasks(tasks);
        self
    }

    fn enqueue_tasks(&mut self, tasks: Vec<Box<dyn Task>>) {
        for task in tasks {
            if task.runs_at_access_log() {
                self.access_log_task_queue.push(task);
            } else {
                self.task_queue.push(task);
            }
        }
    }

    pub fn with_teardown_tasks(mut self, tasks: Vec<Box<dyn TeardownAction>>) -> Self {
        self.teardown_tasks = tasks;
        self
    }

    pub fn is_terminated(&self) -> bool {
        self.terminated
    }

    fn replace_deferred_with_noop(&mut self) {
        // map existing deferred tasks to no-op consumers
        let deferred = std::mem::take(&mut self.deferred_tasks);
        self.deferred_tasks = deferred
            .into_iter()
            .map(|(token_id, task)| {
                let is_guard = task.is_guard();
                // Create a new PendingTask with no-op processor
                let pending = Box::new(PendingTask::new(
                    task.id().to_string(),
                    Box::new(noop_response_processor(token_id, is_guard)),
                    is_guard,
                )) as Box<dyn Task>;
                (token_id, pending)
            })
            .collect();
    }

    fn execute_teardown(&mut self) {
        self.replace_deferred_with_noop();
        for action in self.teardown_tasks.drain(..) {
            match action.execute(&mut self.ctx) {
                TeardownOutcome::Done => {}
                TeardownOutcome::Deferred(token_id) => {
                    // Create a no-op PendingTask for this deferred teardown action
                    // Teardown tasks (trace export) are currently not guards
                    let pending = Box::new(PendingTask::new(
                        format!("teardown_{}", token_id),
                        Box::new(noop_response_processor(token_id, false)),
                        false,
                    ));
                    if self.deferred_tasks.insert(token_id, pending).is_some() {
                        error!("Duplicate token_id during teardown: {}", token_id);
                    }
                }
            }
        }
    }

    fn process_tasks(&mut self, tasks: Vec<Box<dyn Task>>) -> ProcessOutcome {
        for task in tasks {
            if task.runs_at_access_log() {
                self.access_log_task_queue.push(task);
                continue;
            }

            if task
                .dependencies()
                .iter()
                .any(|dep| !self.settled_tasks.contains(dep))
            {
                self.task_queue.push(task);
                continue;
            }

            let task_id = task.id().to_string();
            match task.apply(&mut self.ctx) {
                TaskOutcome::Done => {
                    self.settled_tasks.insert(task_id);
                }
                TaskOutcome::Deferred { token_id, pending } => {
                    if self.deferred_tasks.insert(token_id, pending).is_some() {
                        error!("Duplicate token_id: {}", token_id);
                    }
                }
                TaskOutcome::Requeued(tasks) => {
                    self.enqueue_tasks(tasks);
                }
                TaskOutcome::Failed => {
                    error!("Task failed: {:?}", task_id);
                    self.settled_tasks.insert(task_id);
                }
                TaskOutcome::Terminate(terminal_task) => {
                    terminal_task.apply(&mut self.ctx);
                    self.task_queue.clear();
                    self.terminated = true;
                    return ProcessOutcome::Terminated;
                }
            }
        }
        ProcessOutcome::Continue
    }

    pub fn eval(mut self) -> PipelineState {
        let tasks = std::mem::take(&mut self.task_queue);
        match self.process_tasks(tasks) {
            ProcessOutcome::Terminated => {
                self.execute_teardown();
                return self.into();
            }
            ProcessOutcome::Continue => {}
        }

        if self.task_queue.is_empty()
            && self.deferred_tasks.is_empty()
            && !self.teardown_tasks.is_empty()
        {
            self.execute_teardown();
        }
        self.into()
    }

    /// Runs JSON StoreTasks after Envoy has completed the stream and before access logs emit.
    pub fn eval_access_log(&mut self) {
        self.ctx.invalidate_attribute_cache();
        let tasks = std::mem::take(&mut self.access_log_task_queue);
        for task in tasks {
            let task_id = task.id().to_string();
            if task
                .dependencies()
                .iter()
                .any(|dep| !self.settled_tasks.contains(dep))
            {
                error!(
                    "Skipping access-log task with unsettled dependencies: {}",
                    task_id
                );
                continue;
            }

            match task.apply(&mut self.ctx) {
                TaskOutcome::Done => {
                    self.settled_tasks.insert(task_id);
                }
                TaskOutcome::Failed => {
                    error!("Access-log task failed: {}", task_id);
                    self.settled_tasks.insert(task_id);
                }
                TaskOutcome::Requeued(_) => {
                    error!(
                        "Access-log task remained pending and was skipped: {}",
                        task_id
                    );
                }
                TaskOutcome::Deferred { .. } => {
                    error!(
                        "Access-log task cannot make an asynchronous call: {}",
                        task_id
                    );
                }
                TaskOutcome::Terminate(_) => {
                    error!(
                        "Access-log task cannot terminate response processing: {}",
                        task_id
                    );
                }
            }
        }
    }

    pub fn digest(
        mut self,
        token_id: u32,
        status_code: u32,
        response_size: usize,
    ) -> PipelineState {
        if let Some(pending) = self.deferred_tasks.remove(&token_id) {
            match self.ctx.set_grpc_response_data(status_code, response_size) {
                Ok(_) => {}
                Err(err) => error!("Failed to set gRPC response data: {}", err),
            };
            let task_id = pending.id().to_string();
            match pending.apply(&mut self.ctx) {
                TaskOutcome::Done => {
                    self.settled_tasks.insert(task_id);
                }
                TaskOutcome::Requeued(tasks) => {
                    self.settled_tasks.insert(task_id);
                    for task in tasks.into_iter().rev() {
                        if task.runs_at_access_log() {
                            self.access_log_task_queue.insert(0, task);
                        } else {
                            self.task_queue.insert(0, task);
                        }
                    }
                }
                TaskOutcome::Deferred { token_id, pending } => {
                    if self.deferred_tasks.insert(token_id, pending).is_some() {
                        error!("Duplicate token_id: {}", token_id);
                    }
                }
                TaskOutcome::Failed => {
                    error!("Failed to process response for token_id: {}", token_id);
                    self.settled_tasks.insert(task_id);
                }
                TaskOutcome::Terminate(terminal_task) => {
                    terminal_task.apply(&mut self.ctx);
                    self.task_queue.clear();
                    self.terminated = true;
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
        self.ctx.barrier.is_tripped()
    }

    pub fn only_access_log_tasks_pending(&self) -> bool {
        !self.access_log_task_queue.is_empty()
            && self.task_queue.is_empty()
            && self.deferred_tasks.is_empty()
    }

    pub fn has_access_log_tasks(&self) -> bool {
        !self.access_log_task_queue.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kuadrant::pipeline::tasks::{FailureModeTask, TracingDecoratorTask};
    use crate::kuadrant::MockWasmHost;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    fn create_test_context() -> ReqRespCtx {
        let mock_host = MockWasmHost::new();
        ReqRespCtx::new(Arc::new(mock_host))
    }

    fn token_id_for(id: &str) -> u32 {
        id.as_bytes()
            .iter()
            .fold(0u32, |acc, &b| acc.wrapping_mul(31).wrapping_add(b as u32))
    }

    struct MockGuardTask {
        id: String,
        dependencies: Vec<String>,
        is_guard: bool,
        complete_outcome: TaskOutcome,
    }

    impl MockGuardTask {
        fn new(id: &str, dependencies: Vec<&str>, is_guard: bool) -> Self {
            Self {
                id: id.to_string(),
                dependencies: dependencies.into_iter().map(|s| s.to_string()).collect(),
                is_guard,
                complete_outcome: TaskOutcome::Done,
            }
        }
    }

    impl Task for MockGuardTask {
        fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
            if self.is_guard {
                ctx.barrier.raise();
            }

            let is_guard = self.is_guard;
            let complete_outcome = self.complete_outcome;
            let token_id = self
                .id
                .as_bytes()
                .iter()
                .fold(0u32, |acc, &b| acc.wrapping_mul(31).wrapping_add(b as u32));

            TaskOutcome::Deferred {
                token_id,
                pending: Box::new(PendingTask::new(
                    self.id.clone(),
                    Box::new(move |ctx| {
                        if is_guard {
                            ctx.barrier.lower();
                        }
                        complete_outcome
                    }),
                    is_guard,
                )),
            }
        }

        fn id(&self) -> &str {
            &self.id
        }

        fn dependencies(&self) -> &[String] {
            &self.dependencies
        }

        fn is_guard(&self) -> bool {
            self.is_guard
        }
    }

    #[test]
    fn scenario_auth_guards_upstream() {
        let ctx = create_test_context();
        let pipeline = Pipeline::new(ctx);

        let auth_task = MockGuardTask::new("auth", vec![], true);
        let pipeline = pipeline.with_tasks(vec![Box::new(auth_task)]);

        assert!(
            !pipeline.requires_pause(),
            "Before dispatch: no barrier, should not pause"
        );

        match pipeline.eval() {
            PipelineState::InProgress(pipeline) => {
                assert!(
                    pipeline.requires_pause() && pipeline.ctx.barrier.is_tripped(),
                    "After auth dispatched: barrier raised, should pause"
                );

                let state = pipeline.digest(token_id_for("auth"), 0, 0);
                assert!(
                    matches!(
                        state,
                        PipelineState::Completed {
                            should_resume: true
                        }
                    ),
                    "Expected Completed with should_resume=true after auth completes"
                );
            }
            PipelineState::Completed { .. } => {
                unreachable!("Expected InProgress after eval");
            }
        }
    }

    #[test]
    fn scenario_auth_then_ratelimit_chain() {
        let ctx = create_test_context();
        let auth_task = MockGuardTask::new("auth", vec![], true);
        let ratelimit_task = MockGuardTask::new("ratelimit", vec!["auth"], true);

        let pipeline =
            Pipeline::new(ctx).with_tasks(vec![Box::new(auth_task), Box::new(ratelimit_task)]);

        match pipeline.eval() {
            PipelineState::InProgress(pipeline) => {
                assert!(
                    pipeline.requires_pause() && pipeline.ctx.barrier.is_tripped(),
                    "Auth dispatched, barrier=1, should pause"
                );

                match pipeline.digest(token_id_for("auth"), 0, 0) {
                    PipelineState::InProgress(pipeline) => {
                        assert!(
                            pipeline.requires_pause() && pipeline.ctx.barrier.is_tripped(),
                            "Ratelimit dispatched, barrier=1, should pause"
                        );

                        let state = pipeline.digest(token_id_for("ratelimit"), 0, 0);
                        assert!(
                            matches!(state, PipelineState::Completed { .. }),
                            "Expected Completed after ratelimit completes (both tasks done)"
                        );
                    }
                    PipelineState::Completed { .. } => {
                        unreachable!("Expected InProgress after auth completes");
                    }
                }
            }
            PipelineState::Completed { .. } => {
                unreachable!("Expected InProgress after first eval");
            }
        }
    }

    #[test]
    fn scenario_task_with_unmet_deps_doesnt_dispatch() {
        let ctx = create_test_context();
        let ratelimit_task = MockGuardTask::new("ratelimit", vec!["auth"], true);

        let pipeline = Pipeline::new(ctx).with_tasks(vec![Box::new(ratelimit_task)]);

        match pipeline.eval() {
            PipelineState::InProgress(p) => {
                assert!(
                    !p.requires_pause() && !p.ctx.barrier.is_tripped(),
                    "Task with unmet deps doesn't dispatch, no barrier raised"
                );
            }
            PipelineState::Completed { .. } => {
                unreachable!("Expected InProgress with task waiting for deps");
            }
        }
    }

    #[test]
    fn scenario_non_guard_doesnt_block_upstream() {
        let ctx = create_test_context();
        let report_task = MockGuardTask::new("report", vec![], false);

        let pipeline = Pipeline::new(ctx).with_tasks(vec![Box::new(report_task)]);

        match pipeline.eval() {
            PipelineState::InProgress(pipeline) => {
                assert!(
                    !pipeline.requires_pause() && !pipeline.ctx.barrier.is_tripped(),
                    "Non-guard task dispatched but barrier not raised, should not pause"
                );

                let state = pipeline.digest(token_id_for("report"), 0, 0);
                assert!(
                    matches!(state, PipelineState::Completed { .. }),
                    "Expected Completed after report completes (only task done)"
                );
            }
            PipelineState::Completed { .. } => {
                unreachable!("Expected InProgress with deferred task");
            }
        }
    }

    #[test]
    fn scenario_multiple_concurrent_guards() {
        let ctx = create_test_context();
        let auth_task = MockGuardTask::new("auth", vec![], true);
        let custom_guard = MockGuardTask::new("custom", vec![], true);

        let pipeline =
            Pipeline::new(ctx).with_tasks(vec![Box::new(auth_task), Box::new(custom_guard)]);

        match pipeline.eval() {
            PipelineState::InProgress(pipeline) => {
                assert_eq!(pipeline.ctx.barrier.count(), 2, "Both guards dispatched");
                assert!(
                    pipeline.requires_pause() && pipeline.ctx.barrier.is_tripped(),
                    "Both guards dispatched, barrier tripped, should pause"
                );

                match pipeline.digest(token_id_for("auth"), 0, 0) {
                    PipelineState::InProgress(pipeline) => {
                        assert_eq!(pipeline.ctx.barrier.count(), 1, "One guard completed");
                        assert!(
                            pipeline.requires_pause() && pipeline.ctx.barrier.is_tripped(),
                            "One guard completed, barrier still tripped, should still pause"
                        );

                        let state = pipeline.digest(token_id_for("custom"), 0, 0);
                        assert!(
                            matches!(state, PipelineState::Completed { .. }),
                            "Expected Completed after both guards complete (all tasks done)"
                        );
                    }
                    PipelineState::Completed { .. } => {
                        unreachable!("Expected InProgress with one guard still deferred");
                    }
                }
            }
            PipelineState::Completed { .. } => {
                unreachable!("Expected InProgress after both guards dispatch");
            }
        }
    }

    #[test]
    fn scenario_mixed_guard_and_non_guard() {
        let ctx = create_test_context();
        let auth_task = MockGuardTask::new("auth", vec![], true);
        let report_task = MockGuardTask::new("report", vec!["auth"], false);

        let pipeline =
            Pipeline::new(ctx).with_tasks(vec![Box::new(auth_task), Box::new(report_task)]);

        match pipeline.eval() {
            PipelineState::InProgress(pipeline) => {
                assert!(
                    pipeline.requires_pause() && pipeline.ctx.barrier.is_tripped(),
                    "Only auth (guard) raises barrier, should pause"
                );

                match pipeline.digest(token_id_for("auth"), 0, 0) {
                    PipelineState::InProgress(p) => {
                        assert!(
                            !p.requires_pause() && !p.ctx.barrier.is_tripped(),
                            "Report (non-guard) dispatches but doesn't raise barrier, should not pause"
                        );
                    }
                    PipelineState::Completed { .. } => {
                        unreachable!("Expected InProgress after auth completes");
                    }
                }
            }
            PipelineState::Completed { .. } => {
                unreachable!("Expected InProgress after auth dispatch");
            }
        }
    }

    struct MockAccessLogTask {
        id: String,
        runs: Arc<AtomicUsize>,
    }

    impl MockAccessLogTask {
        fn new(id: &str, runs: Arc<AtomicUsize>) -> Self {
            Self {
                id: id.to_string(),
                runs,
            }
        }
    }

    impl Task for MockAccessLogTask {
        fn apply(self: Box<Self>, _ctx: &mut ReqRespCtx) -> TaskOutcome {
            self.runs.fetch_add(1, Ordering::SeqCst);
            TaskOutcome::Done
        }

        fn id(&self) -> &str {
            &self.id
        }

        fn runs_at_access_log(&self) -> bool {
            true
        }
    }

    struct MockDeferredAccessLogChildTask {
        id: String,
        runs: Arc<AtomicUsize>,
    }

    impl Task for MockDeferredAccessLogChildTask {
        fn apply(self: Box<Self>, _ctx: &mut ReqRespCtx) -> TaskOutcome {
            let child = Box::new(MockAccessLogTask::new("access-log-child", self.runs));
            TaskOutcome::Deferred {
                token_id: 42,
                pending: Box::new(PendingTask::new(
                    self.id.clone(),
                    Box::new(move |_ctx| TaskOutcome::Requeued(vec![child])),
                    false,
                )),
            }
        }

        fn id(&self) -> &str {
            &self.id
        }
    }

    #[test]
    fn wrapped_on_reply_access_log_tasks_wait_for_access_log_eval() {
        let runs = Arc::new(AtomicUsize::new(0));
        let task: Box<dyn Task> = Box::new(MockDeferredAccessLogChildTask {
            id: "auth".to_string(),
            runs: runs.clone(),
        });
        let task: Box<dyn Task> = Box::new(FailureModeTask::new(task, false));
        let task: Box<dyn Task> =
            Box::new(TracingDecoratorTask::new("auth".to_string(), task, vec![]));

        let PipelineState::InProgress(pipeline) = Pipeline::new(create_test_context())
            .with_tasks(vec![task])
            .eval()
        else {
            unreachable!("the gRPC action should remain pending")
        };
        let PipelineState::InProgress(mut pipeline) = pipeline.digest(42, 0, 0) else {
            unreachable!("the access-log child should remain pending until on_log")
        };

        assert_eq!(runs.load(Ordering::SeqCst), 0);
        assert!(pipeline.only_access_log_tasks_pending());

        pipeline.eval_access_log();

        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert!(!pipeline.has_access_log_tasks());
    }

    #[test]
    fn access_log_tasks_run_only_during_access_log_eval() {
        let runs = Arc::new(AtomicUsize::new(0));
        let pipeline = Pipeline::new(create_test_context()).with_tasks(vec![Box::new(
            MockAccessLogTask::new("access-log-export", runs.clone()),
        )]);

        let PipelineState::InProgress(mut pipeline) = pipeline.eval() else {
            unreachable!("access-log task should keep the pipeline pending until on_log")
        };

        assert_eq!(runs.load(Ordering::SeqCst), 0);
        assert!(pipeline.only_access_log_tasks_pending());

        pipeline.eval_access_log();

        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert!(!pipeline.has_access_log_tasks());
    }
}
