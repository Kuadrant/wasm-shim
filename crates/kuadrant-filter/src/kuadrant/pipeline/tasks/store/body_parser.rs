use std::sync::Arc;

use cel::Value;

use crate::data::attribute::AttributeError;
use crate::data::cel::{BodyFieldGroup, ExpectedType};
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

/// Resolves a matched candidate's raw JSON content into a [`Value`] that satisfies
/// `expected`, or `None` if it doesn't -- validated before a mismatched value is ever
/// built, rather than built and then rejected. `is_string` records whether `raw` came
/// from a JSON string literal (quotes already stripped) or a bare token (number, bool,
/// null): that distinction is lost once `raw` is just bytes, so callers must track it
/// per candidate as they parse (see `JsonBodyParser`'s callback and
/// `SseBodyParser::finalize`'s `serde_json::Value` match).
///
/// `'number'` keeps its documented leniency (a numeric-looking JSON string still
/// resolves as a number) since it goes through [`parse_json_scalar`] regardless of
/// `is_string`. Every other hint -- including no hint at all -- treats a confirmed
/// JSON string as a string, even if its content looks like a number, bool, or null.
pub(super) fn resolved_value(
    raw: &str,
    is_string: bool,
    expected: Option<ExpectedType>,
) -> Option<Value> {
    match expected {
        Some(ExpectedType::String) => is_string.then(|| Value::String(Arc::new(raw.to_string()))),
        Some(ExpectedType::Number) => {
            let value = parse_json_scalar(raw);
            matches!(value, Value::Int(_) | Value::UInt(_) | Value::Float(_)).then_some(value)
        }
        Some(ExpectedType::Bool) => (!is_string)
            .then(|| parse_json_scalar(raw))
            .filter(|value| matches!(value, Value::Bool(_))),
        None => {
            let value = if is_string {
                Value::String(Arc::new(raw.to_string()))
            } else {
                parse_json_scalar(raw)
            };
            (!matches!(value, Value::Null)).then_some(value)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_hint_accepts_a_numeric_looking_json_string() {
        // The bug: a JSON string "12345" and a bare number 12345 look identical
        // once quotes are stripped, so a naive parse would fold this into an Int
        // and a 'string' hint would never match it.
        assert_eq!(
            resolved_value("12345", true, Some(ExpectedType::String)),
            Some(Value::String(Arc::new("12345".to_string())))
        );
    }

    #[test]
    fn string_hint_rejects_a_bare_number() {
        assert_eq!(
            resolved_value("12345", false, Some(ExpectedType::String)),
            None
        );
    }

    #[test]
    fn number_hint_keeps_its_leniency_for_a_numeric_json_string() {
        // Pre-existing, documented behaviour: 'number' still accepts a numeric
        // string, whether or not it's confirmed to be a JSON string.
        assert_eq!(
            resolved_value("150", true, Some(ExpectedType::Number)),
            Some(Value::Int(150))
        );
        assert_eq!(
            resolved_value("150", false, Some(ExpectedType::Number)),
            Some(Value::Int(150))
        );
    }

    #[test]
    fn number_hint_rejects_a_non_numeric_json_string() {
        assert_eq!(
            resolved_value("gpt-4", true, Some(ExpectedType::Number)),
            None
        );
    }

    #[test]
    fn bool_hint_rejects_a_json_string_that_looks_like_a_bool() {
        // A JSON string "true" is not a bool, even though its bytes match the
        // bare `true` token.
        assert_eq!(resolved_value("true", true, Some(ExpectedType::Bool)), None);
        assert_eq!(
            resolved_value("true", false, Some(ExpectedType::Bool)),
            Some(Value::Bool(true))
        );
    }

    #[test]
    fn untyped_preserves_a_numeric_looking_json_string() {
        // Matches SseBodyParser's existing untyped behaviour: with no hint at
        // all, a confirmed JSON string is never coerced, even if its content
        // looks numeric, boolean, or null.
        assert_eq!(
            resolved_value("150", true, None),
            Some(Value::String(Arc::new("150".to_string())))
        );
        assert_eq!(
            resolved_value("null", true, None),
            Some(Value::String(Arc::new("null".to_string())))
        );
    }

    #[test]
    fn untyped_still_coerces_a_bare_token() {
        assert_eq!(resolved_value("150", false, None), Some(Value::Int(150)));
        assert_eq!(resolved_value("true", false, None), Some(Value::Bool(true)));
    }

    #[test]
    fn untyped_bare_null_is_not_resolved() {
        // A present-but-null value is still "no such value" for the
        // single-pointer function's documented non-null contract.
        assert_eq!(resolved_value("null", false, None), None);
    }

    #[test]
    fn untyped_empty_string_is_resolved_as_a_string() {
        assert_eq!(
            resolved_value("", true, None),
            Some(Value::String(Arc::new(String::new())))
        );
    }
}
