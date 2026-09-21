use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use cel::Value;
use tracing::error;

use super::body_parser::{parse_json_scalar, BodyParser};
use crate::data::attribute::AttributeError;
use crate::data::cel::BodyFieldGroup;
use crate::kuadrant::context::BodyContext;

pub(crate) struct JsonBodyParser {
    groups: Vec<BodyFieldGroup>,
    parser: Option<acutejson::Parser>,
    buffers: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    matched: Arc<Mutex<HashSet<String>>>,
    extracted: HashMap<String, Value>,
    bytes_consumed: usize,
    complete: bool,
}

impl JsonBodyParser {
    pub fn new(groups: Vec<BodyFieldGroup>) -> Result<Self, AttributeError> {
        let buffers: Arc<Mutex<HashMap<String, Vec<u8>>>> = Arc::new(Mutex::new(HashMap::new()));
        let matched: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
        let results: Arc<Mutex<HashMap<String, Vec<u8>>>> = Arc::clone(&buffers);

        // Every candidate pointer, across every group, is registered independently;
        // a candidate shared by more than one group is only registered (and parsed)
        // once.
        let mut candidates: HashSet<&str> = HashSet::new();
        for group in &groups {
            for candidate in &group.candidates {
                candidates.insert(candidate.as_str());
            }
        }

        let mut builder = acutejson::Builder::new();
        for field in candidates {
            let field_name = field.to_string();
            let field_buffers = Arc::clone(&results);
            let field_matched = Arc::clone(&matched);
            field_buffers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(field_name.clone(), Vec::new());

            builder = match builder.register(field, move |bytes, _is_complete| {
                field_matched
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(field_name.clone());
                let mut bufs = field_buffers.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(buf) = bufs.get_mut(&field_name) {
                    buf.extend_from_slice(bytes);
                } else {
                    error!("Buffer not found for field {}", field_name);
                }
            }) {
                Ok(b) => b,
                Err(e) => {
                    error!("Invalid JSON pointer: {e:?}");
                    return Err(AttributeError::Parse(format!(
                        "Invalid JSON pointer: {e:?}"
                    )));
                }
            };
        }

        Ok(Self {
            groups,
            parser: Some(builder.build()),
            buffers,
            matched,
            extracted: HashMap::new(),
            bytes_consumed: 0,
            complete: false,
        })
    }

    fn finalize_extracted(&mut self) -> Result<(), AttributeError> {
        let buffers = self.buffers.lock().unwrap_or_else(|e| e.into_inner());
        let matched = self.matched.lock().unwrap_or_else(|e| e.into_inner());
        let mut candidate_values: HashMap<&str, Value> = HashMap::new();
        for (field, raw_bytes) in buffers.iter() {
            if matched.contains(field) {
                let raw_value = std::str::from_utf8(raw_bytes).map_err(|e| {
                    AttributeError::Parse(format!("Body field '{field}' is not valid UTF-8: {e}"))
                })?;
                candidate_values.insert(field.as_str(), parse_json_scalar(raw_value));
            }
        }
        for group in &self.groups {
            if self.extracted.contains_key(&group.key) {
                continue;
            }
            if let Some(value) = group.resolve(|candidate| candidate_values.get(candidate)) {
                self.extracted.insert(group.key.clone(), value.clone());
            }
        }
        Ok(())
    }
}

impl BodyParser for JsonBodyParser {
    fn bytes_consumed(&self) -> usize {
        self.bytes_consumed
    }

    fn finalize(&mut self) -> Result<(), AttributeError> {
        if let Some(ref mut parser) = self.parser {
            parser
                .finish()
                .map_err(|e| AttributeError::Parse(format!("JSON finalize error: {e}")))?;
        }
        self.finalize_extracted()?;
        Ok(())
    }

    fn remaining_fields(&self) -> Vec<&BodyFieldGroup> {
        self.groups
            .iter()
            .filter(|g| !self.extracted.contains_key(&g.key))
            .collect()
    }

    fn feed(&mut self, chunk: &[u8]) -> Result<(), AttributeError> {
        self.bytes_consumed += chunk.len();

        if self.complete {
            return Ok(());
        }

        let parser = match self.parser.as_mut() {
            Some(p) => p,
            None => return Err(AttributeError::Parse("Parser not initialized".to_string())),
        };

        match parser.feed(chunk) {
            Ok(acutejson::Status::Done) => {
                self.complete = true;
                self.finalize_extracted()?;
            }
            Ok(acutejson::Status::NeedMore) => {}
            Err(e) => {
                error!("JSON parse error: {e:?}");
                return Err(AttributeError::Parse(format!("JSON parse error: {e:?}")));
            }
        }

        Ok(())
    }

    fn populate(&self, body_ctx: &mut BodyContext) {
        for (field, value) in &self.extracted {
            body_ctx.set_value(field, value.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::cel::ExpectedType;
    use std::sync::Arc;

    fn group(pointer: &str) -> BodyFieldGroup {
        BodyFieldGroup::new(vec![pointer.to_string()], None)
    }

    fn groups(pointers: &[&str]) -> Vec<BodyFieldGroup> {
        pointers.iter().map(|p| group(p)).collect()
    }

    #[test]
    fn single_chunk_extracts_field() {
        let mut parser = JsonBodyParser::new(vec![group("/model")]).unwrap();

        parser.feed(br#"{"model":"gpt-4"}"#).unwrap();

        assert!(parser.remaining_fields().is_empty());

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(
            body_ctx.get_value("/model"),
            Some(&Value::String(Arc::new("gpt-4".to_string())))
        );
    }

    #[test]
    fn chunked_feed_extracts_field() {
        let mut parser = JsonBodyParser::new(vec![group("/stream")]).unwrap();

        parser.feed(br#"{"model":"gpt"#).unwrap();
        let expected = group("/stream");
        assert_eq!(parser.remaining_fields(), vec![&expected]);

        parser.feed(br#"-4","stream":true}"#).unwrap();
        assert!(parser.remaining_fields().is_empty());

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(body_ctx.get_value("/stream"), Some(&Value::Bool(true)));
    }

    #[test]
    fn missing_field_remains_in_remaining() {
        let mut parser = JsonBodyParser::new(vec![group("/missing")]).unwrap();

        parser.feed(br#"{"other":1}"#).unwrap();
        parser.finalize().unwrap();

        let expected = group("/missing");
        assert_eq!(parser.remaining_fields(), vec![&expected]);
    }

    #[test]
    fn multiple_fields_extracted() {
        let mut parser = JsonBodyParser::new(groups(&["/a", "/b"])).unwrap();

        parser.feed(br#"{"a":10,"b":"hello"}"#).unwrap();

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(body_ctx.get_value("/a"), Some(&Value::Int(10)));
        assert_eq!(
            body_ctx.get_value("/b"),
            Some(&Value::String(Arc::new("hello".to_string())))
        );
    }

    #[test]
    fn malformed_json_returns_error() {
        let mut parser = JsonBodyParser::new(vec![group("/field")]).unwrap();

        assert!(parser.feed(b"{not valid json}").is_err());
    }

    #[test]
    fn finalize_catches_truncated_json() {
        let mut parser = JsonBodyParser::new(vec![group("/field")]).unwrap();

        parser.feed(br#"{"field": "#).unwrap();
        assert!(parser.finalize().is_err());
    }

    #[test]
    fn invalid_json_pointer_returns_error() {
        assert!(JsonBodyParser::new(vec![group("no-leading-slash")]).is_err());
    }

    #[test]
    fn nested_field_extracted() {
        let mut parser = JsonBodyParser::new(vec![group("/usage/total_tokens")]).unwrap();

        parser.feed(br#"{"usage":{"total_tokens":42}}"#).unwrap();

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(
            body_ctx.get_value("/usage/total_tokens"),
            Some(&Value::Int(42))
        );
    }

    #[test]
    fn bytes_consumed_tracks_fed_bytes() {
        let mut parser = JsonBodyParser::new(vec![group("/a")]).unwrap();
        assert_eq!(parser.bytes_consumed(), 0);

        parser.feed(br#"{"a""#).unwrap();
        assert_eq!(parser.bytes_consumed(), 4);

        parser.feed(br#":1}"#).unwrap();
        assert_eq!(parser.bytes_consumed(), 7);
    }

    #[test]
    fn empty_string_value_is_extracted() {
        let mut parser = JsonBodyParser::new(vec![group("/name")]).unwrap();

        parser.feed(br#"{"name":""}"#).unwrap();

        assert!(parser.remaining_fields().is_empty());

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(
            body_ctx.get_value("/name"),
            Some(&Value::String(Arc::new(String::new())))
        );
    }

    #[test]
    fn ordered_candidates_first_present_wins() {
        let field = BodyFieldGroup::new(
            vec![
                "/usage/total_tokens".to_string(),
                "/usage/totalTokens".to_string(),
            ],
            None,
        );
        let mut parser = JsonBodyParser::new(vec![field.clone()]).unwrap();

        parser.feed(br#"{"usage":{"totalTokens":7}}"#).unwrap();
        // Only the second candidate is present, so `feed` alone never sees every
        // registered pointer match; `finalize` is what harvests a partial group.
        parser.finalize().unwrap();

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(body_ctx.get_value(&field.key), Some(&Value::Int(7)));
    }

    #[test]
    fn ordered_candidates_earlier_priority_wins_over_later_one() {
        let field = BodyFieldGroup::new(
            vec![
                "/usage/total_tokens".to_string(),
                "/usage/totalTokens".to_string(),
            ],
            None,
        );
        let mut parser = JsonBodyParser::new(vec![field.clone()]).unwrap();

        parser
            .feed(br#"{"usage":{"total_tokens":42,"totalTokens":7}}"#)
            .unwrap();

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(body_ctx.get_value(&field.key), Some(&Value::Int(42)));
    }

    #[test]
    fn untyped_group_skips_a_null_candidate_and_falls_through() {
        // No type hint: a present-but-null first candidate must not stop the
        // fallback chain, since it isn't a "resolved" value either.
        let field = BodyFieldGroup::new(
            vec![
                "/usage/total_tokens".to_string(),
                "/usageMetadata/totalTokenCount".to_string(),
            ],
            None,
        );
        let mut parser = JsonBodyParser::new(vec![field.clone()]).unwrap();

        parser
            .feed(br#"{"usage":{"total_tokens":null},"usageMetadata":{"totalTokenCount":18}}"#)
            .unwrap();

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(body_ctx.get_value(&field.key), Some(&Value::Int(18)));
    }

    #[test]
    fn type_hint_skips_non_matching_candidate() {
        let field = BodyFieldGroup::new(
            vec!["/model".to_string(), "/usage/total_tokens".to_string()],
            Some(ExpectedType::Number),
        );
        let mut parser = JsonBodyParser::new(vec![field.clone()]).unwrap();

        // "/model" resolves first but isn't a number, so the numeric candidate wins.
        parser
            .feed(br#"{"model":"gpt-4","usage":{"total_tokens":42}}"#)
            .unwrap();

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(body_ctx.get_value(&field.key), Some(&Value::Int(42)));
    }

    #[test]
    fn type_hint_with_no_matching_candidate_remains_unresolved() {
        let field = BodyFieldGroup::new(vec!["/model".to_string()], Some(ExpectedType::Number));
        let mut parser = JsonBodyParser::new(vec![field.clone()]).unwrap();

        parser.feed(br#"{"model":"gpt-4"}"#).unwrap();
        parser.finalize().unwrap();

        assert_eq!(parser.remaining_fields(), vec![&field]);
    }
}
