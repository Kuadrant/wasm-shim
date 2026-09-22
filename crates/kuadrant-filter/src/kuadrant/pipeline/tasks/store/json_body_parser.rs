use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use cel::Value;
use tracing::error;

use super::body_parser::{resolved_value, BodyParser};
use crate::data::attribute::AttributeError;
use crate::data::cel::BodyFieldGroup;
use crate::kuadrant::context::BodyContext;

pub(crate) struct JsonBodyParser {
    groups: Vec<BodyFieldGroup>,
    parser: Option<acutejson::Parser>,
    buffers: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    matched: Arc<Mutex<HashSet<String>>>,
    /// Candidates confirmed to have come from a JSON string literal (quotes already
    /// stripped by acutejson before the callback fires), as opposed to a bare number/
    /// bool/null token. A JSON string "12345" and a bare number 12345 deliver
    /// byte-identical content to the callback, so this is the only way to tell them
    /// apart -- see the callback in `new` for how it's derived from acutejson's own
    /// callback contract (a string always fires an `is_complete: false` call first,
    /// except when empty, where total content stays empty).
    string_fields: Arc<Mutex<HashSet<String>>>,
    extracted: HashMap<String, Value>,
    bytes_consumed: usize,
    complete: bool,
}

impl JsonBodyParser {
    pub fn new(groups: Vec<BodyFieldGroup>) -> Result<Self, AttributeError> {
        let buffers: Arc<Mutex<HashMap<String, Vec<u8>>>> = Arc::new(Mutex::new(HashMap::new()));
        let matched: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
        let string_fields: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
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
            let field_is_string = Arc::clone(&string_fields);
            field_buffers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(field_name.clone(), Vec::new());

            builder = match builder.register(field, move |bytes, is_complete| {
                field_matched
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(field_name.clone());
                let mut bufs = field_buffers.lock().unwrap_or_else(|e| e.into_inner());
                let buf_was_empty = bufs.get(&field_name).is_none_or(|b| b.is_empty());
                if !is_complete || (buf_was_empty && bytes.is_empty()) {
                    // A JSON string always fires at least one `is_complete: false`
                    // call for non-empty content; an empty string is the only case
                    // that completes in a single call with no content at all. A bare
                    // number/bool/null token, by contrast, always delivers its full
                    // (non-empty) content in exactly one `is_complete: true` call.
                    field_is_string
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(field_name.clone());
                }
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
            string_fields,
            extracted: HashMap::new(),
            bytes_consumed: 0,
            complete: false,
        })
    }

    fn finalize_extracted(&mut self) -> Result<(), AttributeError> {
        let buffers = self.buffers.lock().unwrap_or_else(|e| e.into_inner());
        let matched = self.matched.lock().unwrap_or_else(|e| e.into_inner());
        let string_fields = self.string_fields.lock().unwrap_or_else(|e| e.into_inner());

        let mut raw_candidates: HashMap<&str, (&str, bool)> = HashMap::new();
        for (field, raw_bytes) in buffers.iter() {
            if matched.contains(field) {
                let raw_value = std::str::from_utf8(raw_bytes).map_err(|e| {
                    AttributeError::Parse(format!("Body field '{field}' is not valid UTF-8: {e}"))
                })?;
                raw_candidates.insert(field.as_str(), (raw_value, string_fields.contains(field)));
            }
        }

        for group in &self.groups {
            if self.extracted.contains_key(&group.key) {
                continue;
            }
            // Computed per group, not once globally: whether a candidate's value
            // satisfies `expected` (and what CEL value it becomes) depends on the
            // group's own type hint, so the same raw candidate can legitimately
            // resolve differently for two groups that happen to share it.
            let mut candidate_values: HashMap<&str, Value> = HashMap::new();
            for candidate in &group.candidates {
                if let Some(&(raw, is_string)) = raw_candidates.get(candidate.as_str()) {
                    if let Some(value) = resolved_value(raw, is_string, group.expected) {
                        candidate_values.insert(candidate.as_str(), value);
                    }
                }
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

        // A registered candidate that a group never ends up using (because
        // some other candidate resolved the group first) still has to stay
        // registered, since we can't know in advance which one will match;
        // `acutejson::Status::Done` therefore won't fire until every
        // registered candidate is seen, not just the ones we still care
        // about. So resolution is attempted after every chunk rather than
        // only on `Done`: a group is done as soon as any of its own
        // candidates resolves, regardless of the other registered paths.
        let feed_result = parser.feed(chunk);
        self.finalize_extracted()?;

        match feed_result {
            Ok(acutejson::Status::Done) => {
                self.complete = true;
                Ok(())
            }
            Ok(acutejson::Status::NeedMore) => Ok(()),
            Err(e) => {
                if self.remaining_fields().is_empty() {
                    // Everything this parser was asked for already resolved
                    // from bytes seen before the error; a malformed or
                    // truncated remainder no longer matters.
                    self.complete = true;
                    Ok(())
                } else {
                    error!("JSON parse error: {e:?}");
                    Err(AttributeError::Parse(format!("JSON parse error: {e:?}")))
                }
            }
        }
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
        // Resolution is incremental: the second candidate is the only one
        // present, and its callback fires within this single `feed` call, so
        // the group is already resolved without needing `finalize`.
        assert!(parser.remaining_fields().is_empty());

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(body_ctx.get_value(&field.key), Some(&Value::Int(7)));
    }

    #[test]
    fn first_candidate_wins_when_multiple_resolve_in_the_same_chunk() {
        // Both candidates are present and fully parsed within the same
        // `feed` call. When more than one candidate genuinely resolves at
        // once like this, list order is the deterministic tie-breaker.
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
    fn candidate_seen_first_in_the_stream_wins_over_higher_list_priority() {
        // The group lists "/usage/total_tokens" first, but
        // "/usage/totalTokens" is the one that actually appears (and fully
        // resolves) earlier in the byte stream. Resolution is "whoever
        // arrives first, wins" -- not list order -- so the group locks onto
        // 7 as soon as it's seen, before "/usage/total_tokens" ever shows up.
        let field = BodyFieldGroup::new(
            vec![
                "/usage/total_tokens".to_string(),
                "/usage/totalTokens".to_string(),
            ],
            None,
        );
        let mut parser = JsonBodyParser::new(vec![field.clone()]).unwrap();

        parser.feed(br#"{"usage":{"totalTokens":7,"#).unwrap();
        assert!(parser.remaining_fields().is_empty());

        // The higher list-priority candidate arrives afterwards; it must not
        // override the value already resolved from the first one seen.
        parser.feed(br#""total_tokens":42}}"#).unwrap();

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(body_ctx.get_value(&field.key), Some(&Value::Int(7)));
    }

    #[test]
    fn resolved_group_survives_malformed_bytes_from_a_never_used_candidate() {
        // A second candidate is registered but never appears; since
        // `acutejson::Status::Done` only fires once every registered
        // candidate is seen, the parser keeps parsing (and can hit the
        // trailing garbage below) even though the group we actually care
        // about already resolved via the first candidate.
        let field = BodyFieldGroup::new(
            vec![
                "/usage/total_tokens".to_string(),
                "/never/present".to_string(),
            ],
            None,
        );
        let mut parser = JsonBodyParser::new(vec![field.clone()]).unwrap();

        let result = parser.feed(br#"{"usage":{"total_tokens":42}}TRAILING_GARBAGE"#);
        assert!(result.is_ok());
        assert!(parser.remaining_fields().is_empty());

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

    #[test]
    fn string_hint_resolves_a_numeric_looking_json_string() {
        // Regression test: a JSON string "12345" and a bare number 12345 look
        // byte-identical once acutejson strips the quotes, so a naive parse would
        // fold this into an Int and a 'string' hint would never match it.
        let field = BodyFieldGroup::new(vec!["/x".to_string()], Some(ExpectedType::String));
        let mut parser = JsonBodyParser::new(vec![field.clone()]).unwrap();

        parser.feed(br#"{"x":"12345"}"#).unwrap();

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(
            body_ctx.get_value(&field.key),
            Some(&Value::String(Arc::new("12345".to_string())))
        );
    }

    #[test]
    fn string_hint_rejects_a_bare_number() {
        let field = BodyFieldGroup::new(vec!["/x".to_string()], Some(ExpectedType::String));
        let mut parser = JsonBodyParser::new(vec![field.clone()]).unwrap();

        parser.feed(br#"{"x":12345}"#).unwrap();
        parser.finalize().unwrap();

        assert_eq!(parser.remaining_fields(), vec![&field]);
    }

    #[test]
    fn untyped_call_preserves_a_numeric_looking_json_string() {
        // Consistency with SseBodyParser: with no type hint at all, a confirmed
        // JSON string is never coerced, even when its content looks numeric.
        let field = BodyFieldGroup::new(vec!["/x".to_string()], None);
        let mut parser = JsonBodyParser::new(vec![field.clone()]).unwrap();

        parser.feed(br#"{"x":"12345"}"#).unwrap();

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(
            body_ctx.get_value(&field.key),
            Some(&Value::String(Arc::new("12345".to_string())))
        );
    }

    #[test]
    fn same_candidate_resolves_differently_for_groups_with_different_hints() {
        // The same raw value can't be "both" a number and a string, but the two
        // hints are asking two different, both-legitimate questions about it:
        // "coerce this to a number if reasonable" vs. "give me it as a string".
        // Both groups share the underlying candidate, extracted only once.
        let number_group = BodyFieldGroup::new(vec!["/x".to_string()], Some(ExpectedType::Number));
        let string_group = BodyFieldGroup::new(vec!["/x".to_string()], Some(ExpectedType::String));
        let mut parser =
            JsonBodyParser::new(vec![number_group.clone(), string_group.clone()]).unwrap();

        parser.feed(br#"{"x":"12345"}"#).unwrap();

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(
            body_ctx.get_value(&number_group.key),
            Some(&Value::Int(12345))
        );
        assert_eq!(
            body_ctx.get_value(&string_group.key),
            Some(&Value::String(Arc::new("12345".to_string())))
        );
    }
}
