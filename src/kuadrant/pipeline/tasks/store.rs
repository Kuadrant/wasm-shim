use tracing::error;

use crate::kuadrant::pipeline::tasks::{Task, TaskOutcome};
use crate::kuadrant::ReqRespCtx;
use crate::services::MessageConverter;
use cel::Value;
use prost::Message;
use prost_reflect::ReflectMessage;
use std::fmt;

#[derive(Debug)]
enum StoreError {
    UnsupportedType(String),
    ProtobufConversion(String),
    ProtobufEncoding(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::UnsupportedType(msg) => write!(f, "Unsupported value type: {}", msg),
            StoreError::ProtobufConversion(msg) => write!(f, "Protobuf conversion failed: {}", msg),
            StoreError::ProtobufEncoding(msg) => write!(f, "Protobuf encoding failed: {}", msg),
        }
    }
}

impl From<crate::services::ConversionError> for StoreError {
    fn from(e: crate::services::ConversionError) -> Self {
        StoreError::ProtobufConversion(e.to_string())
    }
}

impl From<prost::EncodeError> for StoreError {
    fn from(e: prost::EncodeError) -> Self {
        StoreError::ProtobufEncoding(e.to_string())
    }
}

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

    fn value_to_bytes(value: &Value) -> Result<Vec<u8>, StoreError> {
        match value {
            Value::Struct(s) if s.name() == "google.protobuf.Struct" => {
                let descriptor = prost_types::Struct::default().descriptor();
                let dynamic_msg = MessageConverter::cel_to_dynamic_message(value, &descriptor)?;
                let mut bytes = Vec::new();
                dynamic_msg.encode(&mut bytes)?;
                Ok(bytes)
            }
            Value::String(s) => Ok(s.to_string().into_bytes()),
            Value::Int(n) => Ok(n.to_string().into_bytes()),
            Value::UInt(n) => Ok(n.to_string().into_bytes()),
            Value::Float(n) => Ok(n.to_string().into_bytes()),
            Value::Bool(b) => Ok(b.to_string().into_bytes()),
            Value::Null => Ok(Vec::new()),
            _ => Err(StoreError::UnsupportedType(format!("{:?}", value))),
        }
    }
}

impl Task for StoreTask {
    #[tracing::instrument(name = "store", skip(self, ctx), level = tracing::Level::TRACE)]
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        if self.export_to_host {
            match Self::value_to_bytes(&self.value) {
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
