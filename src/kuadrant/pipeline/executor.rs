use log::error;

use crate::kuadrant::{
    pipeline::tasks::{Task, TaskOutcome},
    ReqRespCtx,
};
use std::collections::{BTreeMap, HashSet};

pub struct Pipeline {
    pub ctx: ReqRespCtx,
    task_queue: Vec<Box<dyn Task>>,
    deferred_tasks: BTreeMap<u32, Box<dyn Task>>,
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

    pub fn with_tasks(mut self, tasks: Vec<Box<dyn Task>>) -> Self {
        self.task_queue = tasks;
        self
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
                    self.deferred_tasks.clear();
                    return None;
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
                    self.deferred_tasks.clear();
                    return None;
                }
            }
        } else {
            error!("token_id: {} not found", token_id);
        }

        self.eval()
    }

    pub fn requires_pause(&self) -> bool {
        let has_blocking_deferred_tasks = self
            .deferred_tasks
            .values()
            .any(|task| task.pauses_filter(&self.ctx));

        let has_blocking_queued_tasks = self
            .task_queue
            .iter()
            .any(|task| task.pauses_filter(&self.ctx));

        has_blocking_deferred_tasks || has_blocking_queued_tasks
    }
}
