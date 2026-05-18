use tracing::error;

use crate::kuadrant::pipeline::tasks::{Task, TaskOutcome};
use crate::kuadrant::ReqRespCtx;
use cel::Value;

pub struct StoreTask {
    path: String,
    value: Value,
}

impl StoreTask {
    pub fn new(path: String, value: Value) -> Self {
        Self { path, value }
    }

    fn value_to_bytes(value: &Value) -> Vec<u8> {
        match value {
            Value::String(s) => s.to_string().into_bytes(),
            Value::Int(n) => n.to_string().into_bytes(),
            Value::UInt(n) => n.to_string().into_bytes(),
            Value::Float(n) => n.to_string().into_bytes(),
            Value::Bool(b) => b.to_string().into_bytes(),
            Value::Null => Vec::new(),
            _ => format!("{:?}", value).into_bytes(),
        }
    }
}

impl Task for StoreTask {
    #[tracing::instrument(name = "store", skip(self, ctx), level = tracing::Level::TRACE)]
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        let bytes = Self::value_to_bytes(&self.value);
        if let Err(e) = ctx.set_attribute(&self.path, &bytes) {
            error!("Failed to store attribute {}: {:?}", self.path, e);
            TaskOutcome::Failed
        } else {
            TaskOutcome::Done
        }
    }
}
