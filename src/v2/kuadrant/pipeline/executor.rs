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
        self.task_queue = self
            .task_queue
            .drain(..)
            .filter_map(|mut task| {
                #[allow(deprecated)]
                match task.prepare(&mut self.ctx) {
                    TaskOutcome::Done => {}
                    TaskOutcome::Failed => {
                        error!("Task preparation failed: {:?}", task.id());
                        return None;
                    }
                    _ => {
                        error!("Unexpected TaskOutcome from prepare");
                        return None;
                    }
                }

                loop {
                    if task
                        .dependencies()
                        .iter()
                        .any(|dep| !self.completed_tasks.contains(dep))
                    {
                        return Some(task);
                    }

                    let task_id = task.id();
                    match task.apply(&mut self.ctx) {
                        TaskOutcome::Done => {
                            if let Some(id) = task_id {
                                self.completed_tasks.insert(id);
                            }
                            return None;
                        }
                        TaskOutcome::Continue(next_task) => {
                            task = next_task;
                        }
                        TaskOutcome::Deferred { token_id, pending } => {
                            if let Some(id) = pending.task_id() {
                                self.completed_tasks.insert(id.clone());
                            }
                            if self.deferred_tasks.insert(token_id, pending).is_some() {
                                error!("Duplicate token_id={}", token_id);
                            }
                            return None;
                        }
                        TaskOutcome::Requeued(task) => return Some(task),
                        TaskOutcome::Failed => todo!("Handle failed task"),
                    }
                }
            })
            .collect();

        if self.deferred_tasks.is_empty() && self.task_queue.is_empty() {
            None
        } else {
            Some(self)
        }
    }

    pub fn digest(&mut self, token_id: u32, response: Vec<u8>) {
        if let Some(pending) = self.deferred_tasks.remove(&token_id) {
            let tasks = pending.process_response(response);
            // todo(refactor): error handling
            self.task_queue.extend(tasks);
        } else {
            error!("token_id={} not found", token_id);
        }
    }

    pub fn is_blocked(&self) -> bool {
        self.deferred_tasks.values().any(PendingTask::is_blocking)
    }
}
