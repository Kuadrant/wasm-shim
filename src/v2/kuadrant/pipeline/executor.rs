use log::error;

use crate::v2::kuadrant::ReqRespCtx;
use std::collections::BTreeMap;

pub struct Pipeline {
    ctx: ReqRespCtx,
    task_queue: Vec<Box<dyn Task>>,
    deferred_tasks: BTreeMap<usize, PendingTask>,
}

impl Pipeline {
    pub fn new(ctx: ReqRespCtx) -> Self {
        Self {
            ctx,
            task_queue: Vec::new(),
            deferred_tasks: BTreeMap::new(),
        }
    }

    pub fn eval(mut self) -> Option<Self> {
        self.task_queue = self
            .task_queue
            .drain(..)
            .filter_map(|task| match task.apply(&mut self.ctx) {
                TaskOutcome::Done => None,
                TaskOutcome::Deferred { token_id, pending } => {
                    if self.deferred_tasks.insert(token_id, pending).is_some() {
                        error!("Duplicate token_id={}", token_id);
                    }
                    None
                }
                TaskOutcome::Requeued(task) => Some(task),
            })
            .collect();

        if self.deferred_tasks.is_empty() && self.task_queue.is_empty() {
            None
        } else {
            Some(self)
        }
    }

    pub fn digest(&mut self, token_id: usize, _response: Vec<u8>) {
        if let Some(_pending) = self.deferred_tasks.remove(&token_id) {
            // todo(adam-cattermole): Process the response
            // if let Some(task) = pending.process_response(response) {
            //     match task.apply(&mut self.ctx) {
            //         TaskOutcome::Done => {}
            //         TaskOutcome::Deferred { token_id, pending } => {
            //             if self.deferred_tasks.insert(token_id, pending).is_some() {
            //                 panic!("Duplicate token_id={}", token_id);
            //             }
            //         }
            //         TaskOutcome::Requeued(task) => self.task_queue.push(task),
            //     }
            // };
        } else {
            error!("token_id={} not found", token_id);
        }
    }

    pub fn is_blocked(&self) -> bool {
        self.deferred_tasks.values().any(PendingTask::is_blocking)
    }
}

//todo(adam-cattermole): these are temporary and will be removed
// or moved to the tasks module
pub trait Task {
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome;
}

pub enum TaskOutcome {
    Done,
    Deferred {
        token_id: usize,
        pending: PendingTask,
    },
    Requeued(Box<dyn Task>),
}

pub struct PendingTask {
    is_blocking: bool,
}

impl PendingTask {
    pub fn is_blocking(&self) -> bool {
        self.is_blocking
    }
}
