use cel::Value;

use crate::data::attribute::AttributeError;
use crate::data::cel::BodyFieldGroup;
use crate::kuadrant::context::BodyContext;

pub(super) trait BodyParser {
    fn feed(&mut self, chunk: &[u8]) -> Result<(), AttributeError>;
    fn finalize(&mut self) -> Result<(), AttributeError>;
    /// Groups with no resolved value yet. Returned as the original
    /// [`BodyFieldGroup`] (not just its canonical key) so callers can report
    /// the actual candidate pointers a user configured, rather than the
    /// internal, non-human-readable canonical key.
    fn remaining_fields(&self) -> Vec<&BodyFieldGroup>;
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
