use log::error;

use crate::v2::kuadrant::{
    pipeline::tasks::{PendingTask, Task, TaskOutcome},
    ReqRespCtx,
};
use std::collections::{BTreeMap, HashSet};

pub struct Pipeline {
    ctx: ReqRespCtx,
    task_queue: Vec<Box<dyn Task>>,
    deferred_tasks: BTreeMap<u32, PendingTask>,
    completed_tasks: HashSet<String>,
}

impl Pipeline {
    pub fn new(ctx: ReqRespCtx) -> Self {
        Self {
            ctx,
            task_queue: Vec::new(),
            deferred_tasks: BTreeMap::new(),
            completed_tasks: HashSet::new(),
        }
    }

    pub fn eval(mut self) -> Option<Self> {
        let tasks_to_process: Vec<_> = self.task_queue.drain(..).collect();

        for task in tasks_to_process {
            #[allow(deprecated)]
            match task.prepare(&mut self.ctx) {
                TaskOutcome::Done => {}
                TaskOutcome::Failed => {
                    error!("Task preparation failed: {:?}", task.id());
                    continue;
                }
                TaskOutcome::Requeued(tasks) => {
                    self.task_queue.extend(tasks);
                    continue;
                }
                _ => {
                    error!("Unexpected TaskOutcome from prepare");
                    continue;
                }
            }

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
                    if let Some(id) = pending.task_id() {
                        self.completed_tasks.insert(id.clone());
                    }
                    if self.deferred_tasks.insert(token_id, pending).is_some() {
                        error!("Duplicate token_id={}", token_id);
                    }
                }
                TaskOutcome::Requeued(tasks) => {
                    self.task_queue.extend(tasks);
                }
                TaskOutcome::Failed => {
                    // todo(refactor): error handling
                    error!("Task failed: {:?}", task_id);
                }
            }
        }

        if self.deferred_tasks.is_empty() && self.task_queue.is_empty() {
            None
        } else {
            Some(self)
        }
    }

    pub fn digest(mut self, token_id: u32, status_code: u32, response_size: usize) -> Option<Self> {
        if let Some(pending) = self.deferred_tasks.remove(&token_id) {
            match pending.process_response(&mut self.ctx, status_code, response_size) {
                TaskOutcome::Done => {}
                TaskOutcome::Requeued(tasks) => {
                    for task in tasks.into_iter().rev() {
                        self.task_queue.insert(0, task);
                    }
                }
                TaskOutcome::Deferred { token_id, pending } => {
                    if self.deferred_tasks.insert(token_id, pending).is_some() {
                        error!("Duplicate token_id={}", token_id);
                    }
                }
                TaskOutcome::Failed => {
                    // todo(refactor): error handling
                    error!("Failed to process response for token_id={}", token_id);
                }
            }
        } else {
            error!("token_id={} not found", token_id);
        }

        self.eval()
    }

    pub fn is_blocked(&self) -> bool {
        self.deferred_tasks.values().any(PendingTask::is_blocking)
    }
}
