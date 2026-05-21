use tracing::error;

use crate::kuadrant::pipeline::tasks::{Task, TaskOutcome};
use crate::kuadrant::ReqRespCtx;
use crate::services::MessageConverter;
use cel::Value;

pub struct StoreTask {
    path: String,
    value: Value,
    export_to_host: bool,
}

impl StoreTask {
    pub fn new(path: String, value: Value, export_to_host: bool) -> Self {
        Self {
            path,
            value,
            export_to_host,
        }
    }
}

impl Task for StoreTask {
    #[tracing::instrument(name = "store", skip(self, ctx), level = tracing::Level::TRACE)]
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        if self.export_to_host {
            match MessageConverter::cel_value_to_bytes(&self.value) {
                Ok(bytes) => {
                    if let Err(e) = ctx.set_attribute(&self.path, &bytes) {
                        error!("Failed to store attribute {}: {:?}", self.path, e);
                        return TaskOutcome::Failed;
                    }
                }
                Err(e) => {
                    error!(
                        "Failed to convert value to bytes for '{}': {}",
                        self.path, e
                    );
                    return TaskOutcome::Failed;
                }
            }
        }
        ctx.store_value(self.path, self.value);
        TaskOutcome::Done
    }
}
