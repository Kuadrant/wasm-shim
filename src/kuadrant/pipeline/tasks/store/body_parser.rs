use cel::Value;

use crate::data::attribute::AttributeError;
use crate::kuadrant::context::BodyContext;

pub(super) trait BodyParser {
    fn feed(&mut self, chunk: &[u8]) -> Result<(), AttributeError>;
    fn finalize(&mut self);
    fn is_complete(&self) -> bool;
    fn remaining_fields(&self) -> Vec<&String>;
    fn populate(&self, body_ctx: &mut BodyContext);
    fn bytes_consumed(&self) -> usize;
}

pub(super) fn parse_json_scalar(raw: &str) -> Value {
    if raw == "null" {
        return Value::Null;
    }
    if raw == "true" {
        return Value::Bool(true);
    }
    if raw == "false" {
        return Value::Bool(false);
    }
    if let Ok(i) = raw.parse::<i64>() {
        return Value::Int(i);
    }
    if let Ok(u) = raw.parse::<u64>() {
        return Value::UInt(u);
    }
    if let Ok(f) = raw.parse::<f64>() {
        return Value::Float(f);
    }
    Value::String(std::sync::Arc::new(raw.to_string()))
}
