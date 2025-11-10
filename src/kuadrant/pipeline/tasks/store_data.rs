use log::error;

use crate::kuadrant::pipeline::tasks::{Task, TaskOutcome};
use crate::kuadrant::ReqRespCtx;

pub struct StoreDataTask {
    data: Vec<(String, Vec<u8>)>,
}

impl StoreDataTask {
    pub fn new(data: Vec<(String, Vec<u8>)>) -> Self {
        Self { data }
    }
}

impl Task for StoreDataTask {
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        let mut had_failure = false;

        for (path, value) in &self.data {
            if let Err(e) = ctx.set_attribute(path, value) {
                error!("Failed to store attribute {}: {:?}", path, e);
                had_failure = true;
            }
        }

        if had_failure {
            TaskOutcome::Failed
        } else {
            TaskOutcome::Done
        }
    }
}
