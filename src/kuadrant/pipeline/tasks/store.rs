use tracing::error;

use crate::data::attribute::AttributeState;
use crate::data::cel::Predicate;
use crate::data::Expression;
use crate::kuadrant::pipeline::tasks::{NoopTerminalTask, Task, TaskOutcome};
use crate::kuadrant::ReqRespCtx;
use crate::services::MessageConverter;
use cel::Value;

enum StoreMode {
    Concrete { value: Value },
    Deferred { expression: Expression },
}

pub struct StoreTask {
    predicate: Option<Predicate>,
    mode: StoreMode,
    path: String,
    export_to_host: bool,
    terminal: bool,
}

impl StoreTask {
    pub fn new(path: String, value: Value, export_to_host: bool) -> Self {
        Self {
            predicate: None,
            mode: StoreMode::Concrete { value },
            path,
            export_to_host,
            terminal: false,
        }
    }

    pub fn new_deferred(
        predicate: Predicate,
        expression: Expression,
        path: String,
        export_to_host: bool,
        terminal: bool,
    ) -> Self {
        Self {
            predicate: Some(predicate),
            mode: StoreMode::Deferred { expression },
            path,
            export_to_host,
            terminal,
        }
    }
}

impl Task for StoreTask {
    #[tracing::instrument(name = "store", skip(self, ctx), level = tracing::Level::TRACE)]
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        if let Some(ref predicate) = self.predicate {
            match predicate.test(ctx) {
                Ok(AttributeState::Available(true)) => {}
                Ok(AttributeState::Available(false)) => return TaskOutcome::Done,
                Ok(AttributeState::Pending) => {
                    return TaskOutcome::Requeued(vec![self]);
                }
                Err(e) => {
                    error!("Failed to evaluate predicate: {e:?}");
                    return TaskOutcome::Failed;
                }
            }
        }

        let value = match &self.mode {
            StoreMode::Concrete { value } => value.clone(),
            StoreMode::Deferred { expression } => {
                let mut cel_ctx = cel::Context::default();
                match expression.eval(ctx, &mut cel_ctx) {
                    Ok(AttributeState::Pending) => {
                        error!(
                            "Unexpected pending state in store expression for '{}'",
                            self.path
                        );
                        return TaskOutcome::Failed;
                    }
                    Ok(AttributeState::Available(val)) => val,
                    Err(e) => {
                        error!(
                            "Failed to evaluate store expression for '{}': {e}",
                            self.path
                        );
                        return TaskOutcome::Failed;
                    }
                }
            }
        };

        if self.export_to_host {
            match MessageConverter::cel_value_to_bytes(&value) {
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
        ctx.store_value(self.path.clone(), value);

        if self.terminal {
            TaskOutcome::Terminate(Box::new(NoopTerminalTask))
        } else {
            TaskOutcome::Done
        }
    }
}
