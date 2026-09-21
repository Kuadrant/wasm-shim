use std::collections::HashMap;

use super::body_parser::{resolved_value, BodyParser};
use crate::data::attribute::AttributeError;
use crate::data::cel::BodyFieldGroup;
use crate::kuadrant::context::BodyContext;
use cel::Value;
use core::time::Duration;
use sse_line_parser::RawEventLine;

mod sse_line_parser;

#[derive(Default, PartialEq, Debug)]
pub struct Event {
    /// The event name if given
    pub event: String,
    /// The event data
    pub data: String,
    /// The event id if given
    pub id: String,
    /// Retry duration if given
    pub retry: Option<Duration>,
}

#[derive(Default)]
pub struct EventParser {
    buffer: String,
    event_builder: EventBuilder,
}

impl EventParser {
    pub(crate) fn parse(&mut self, chunk_bytes: Vec<u8>) -> Result<Vec<Event>, String> {
        // taking advantage by the automatic deref coercion.
        // Because String implements Deref<Target=str>,
        // the compiler will automatically convert the string reference (i.e. &String) to a string slice - &str
        self.buffer
            .push_str(&String::from_utf8(chunk_bytes).map_err(|e| e.to_string())?);

        let mut events = Vec::default();

        while let Some(event) = self.parse_one_event()? {
            events.push(event);
        }

        Ok(events)
    }

    fn parse_one_event(&mut self) -> Result<Option<Event>, String> {
        if self.buffer.is_empty() {
            return Ok(None);
        }
        loop {
            match sse_line_parser::line(self.buffer.as_ref()) {
                Ok((rem, next_line)) => {
                    self.event_builder.add(next_line);
                    let consumed = self.buffer.len() - rem.len();
                    let rem = self.buffer.split_off(consumed);
                    self.buffer = rem;
                    if self.event_builder.is_complete {
                        if let Some(event) = self.event_builder.dispatch() {
                            return Ok(Some(event));
                        }
                    }
                }
                Err(nom::Err::Incomplete(_)) => return Ok(None),
                Err(nom::Err::Error(err)) | Err(nom::Err::Failure(err)) => {
                    return Err(err.to_string())
                }
            }
        }
    }
}

#[derive(Default)]
struct EventBuilder {
    event: Event,
    is_complete: bool,
}

impl EventBuilder {
    fn add(&mut self, line: RawEventLine) {
        match line {
            RawEventLine::Field(field, val) => {
                let val = val.unwrap_or("");
                match field {
                    "event" => {
                        self.event.event = val.to_string();
                    }
                    "data" => {
                        if !self.event.data.is_empty() {
                            self.event.data.push('\u{000A}');
                        }
                        self.event.data.push_str(val);
                    }
                    "id" if !val.contains('\u{0000}') => self.event.id = val.to_string(),
                    "retry" => {
                        if let Ok(val) = val.parse::<u64>() {
                            self.event.retry = Some(Duration::from_millis(val))
                        }
                    }
                    _ => {}
                }
            }
            RawEventLine::Comment(_) => {}
            RawEventLine::Empty => self.is_complete = true,
        }
    }

    fn dispatch(&mut self) -> Option<Event> {
        let builder = core::mem::take(self);
        let mut event = builder.event;

        if event.data.is_empty() {
            return None;
        }

        if sse_line_parser::is_lf(event.data.chars().next_back().unwrap_or(' ')) {
            event.data.pop();
        }

        if event.event.is_empty() {
            event.event = "message".to_string();
        }

        Some(event)
    }
}

/// The quote-delimited leaf-key substring for a JSON Pointer candidate: the last
/// reference token, RFC 6901-unescaped, wrapped in the literal quote characters
/// that bound a JSON object key (`/usage/total_tokens` -> `"total_tokens"`).
/// Requiring both quotes is deliberate: it's what stops a shorter key from
/// spuriously matching as a substring of an unrelated, longer key that happens to
/// share a suffix (`input_tokens` is a suffix of Anthropic's
/// `cache_read_input_tokens`, but the quote-bounded `"input_tokens"` is not found
/// inside `"cache_read_input_tokens"`, since that key's only two quote characters
/// are 24 characters apart, not 13).
///
/// Only works for object-key-shaped pointers, matching the RFC's own scope: a
/// pointer ending in an array index (e.g. `/items/0`) has no textual key to scan
/// for at all, since array elements aren't matched by name in the raw JSON text.
/// `None` here means "can't derive a leaf key," not "candidate can't match" --
/// callers must not treat it as proof of absence.
fn leaf_key_substring(pointer: &str) -> Option<String> {
    let last = pointer.rsplit('/').next()?;
    if last.is_empty() || last.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let unescaped = last.replace("~1", "/").replace("~0", "~");
    Some(format!("\"{unescaped}\""))
}

pub(crate) struct SseBodyParser {
    groups: Vec<BodyFieldGroup>,
    /// `leaf_keys[i]` holds the leaf-key substrings for `groups[i].candidates`,
    /// precomputed once so `feed()` never has to derive them per event. A
    /// candidate with no derivable leaf key (e.g. one ending in an array
    /// index) is stored as `""` rather than dropped: `str::contains("")` is
    /// always true, so such a candidate can never cause the scan to
    /// (incorrectly) prove its group absent from an event -- that group's
    /// pre-filter degrades to "always parse" instead of silently starving
    /// the candidate.
    leaf_keys: Vec<Vec<String>>,
    event_parser: EventParser,
    extracted: HashMap<String, Value>,
    complete: bool,
}

impl SseBodyParser {
    pub fn new(groups: Vec<BodyFieldGroup>) -> Self {
        let leaf_keys = groups
            .iter()
            .map(|group| {
                group
                    .candidates
                    .iter()
                    .map(|candidate| leaf_key_substring(candidate).unwrap_or_default())
                    .collect()
            })
            .collect();
        Self {
            groups,
            leaf_keys,
            event_parser: EventParser::default(),
            extracted: HashMap::new(),
            complete: false,
        }
    }

    /// Cheap `str::contains` pre-filter against every group's leaf keys, then --
    /// only on a match -- a single JSON parse of this one event, followed by a
    /// list-order candidate walk for every group that matched. This is the
    /// provider-agnostic replacement for the old "look at the second-to-last
    /// event" heuristic: no knowledge of *which* provider or event-naming
    /// convention is in play is needed, since every event is treated identically.
    ///
    /// A group already in `self.extracted` is skipped entirely: matching
    /// `JsonBodyParser`, the first event (anywhere in the stream, regardless of
    /// which `feed()` call it lands in) to resolve a group wins, permanently --
    /// no later event, however high-priority or however it re-reports the same
    /// candidate, can ever change it.
    fn process_event(&mut self, data: &str) {
        let matched_groups: Vec<usize> = self
            .leaf_keys
            .iter()
            .enumerate()
            .filter(|(i, keys)| {
                !self.extracted.contains_key(&self.groups[*i].key)
                    && keys.iter().any(|key| data.contains(key.as_str()))
            })
            .map(|(i, _)| i)
            .collect();
        if matched_groups.is_empty() {
            return;
        }

        let json: serde_json::Value = match serde_json::from_str(data) {
            Ok(json) => json,
            // A leaf key matched, but this event's data isn't actually valid
            // JSON (e.g. a truncated chunk, or a coincidental substring hit in
            // non-JSON content). Skip just this event rather than failing the
            // whole task -- a later event may still resolve the field(s) this
            // one merely looked like it might concern.
            Err(_) => return,
        };

        for group_idx in matched_groups {
            let group = &self.groups[group_idx];
            let mut candidate_values: HashMap<&str, Value> = HashMap::new();
            for candidate in &group.candidates {
                if candidate_values.contains_key(candidate.as_str()) {
                    continue;
                }
                if let Some(value) = json.pointer(candidate) {
                    let (raw, is_string) = match value {
                        serde_json::Value::String(s) => (s.clone(), true),
                        other => (other.to_string(), false),
                    };
                    if let Some(cel_value) = resolved_value(&raw, is_string, group.expected) {
                        candidate_values.insert(candidate.as_str(), cel_value);
                    }
                }
            }

            if let Some(value) = group.resolve(|candidate| candidate_values.get(candidate)) {
                self.extracted.insert(group.key.clone(), value.clone());
            }
        }
    }

    #[cfg(test)]
    fn is_complete(&self) -> bool {
        self.complete
    }
}

impl BodyParser for SseBodyParser {
    fn bytes_consumed(&self) -> usize {
        0
    }

    fn finalize(&mut self) -> Result<(), AttributeError> {
        self.complete = true;
        Ok(())
    }

    fn remaining_fields(&self) -> Vec<&BodyFieldGroup> {
        self.groups
            .iter()
            .filter(|g| !self.extracted.contains_key(&g.key))
            .collect()
    }

    fn feed(&mut self, chunk: &[u8]) -> Result<(), AttributeError> {
        if self.complete {
            return Ok(());
        }

        let events = self
            .event_parser
            .parse(chunk.to_vec())
            .map_err(|e| AttributeError::Parse(format!("SSE parse error: {e}")))?;

        for event in &events {
            self.process_event(&event.data);
        }

        Ok(())
    }

    /// Exposes whatever's currently in `extracted` to the owning `StoreTask`,
    /// which is called after every chunk. The first event -- anywhere in the
    /// stream -- in which a candidate for a field resolves wins, permanently:
    /// `process_event` never revisits a group once it's in `extracted`, so no
    /// later event can override it, whether via a higher-priority candidate or
    /// a fresh value for the same candidate. There is no same-candidate
    /// progressive-update support -- once resolved, it's done.
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

    fn group(pointer: &str) -> BodyFieldGroup {
        BodyFieldGroup::new(vec![pointer.to_string()], None)
    }

    fn groups(pointers: &[&str]) -> Vec<BodyFieldGroup> {
        pointers.iter().map(|p| group(p)).collect()
    }

    #[test]
    fn test_one_complete_event() {
        let buf = String::from("data: foo\n\n");
        let mut event_parser = EventParser::default();

        let events = event_parser
            .parse(buf.into())
            .expect("should not return parsing error");
        assert_eq!(
            events,
            vec![Event {
                event: "message".to_string(),
                data: "foo".to_string(),
                ..Default::default()
            }]
        );
    }

    #[test]
    fn test_two_complete_events() {
        let buf = String::from("data: first event\n\ndata: second event\n\n");
        let mut event_parser = EventParser::default();

        let events = event_parser
            .parse(buf.into())
            .expect("should not return parsing error");
        assert_eq!(
            events,
            vec![
                Event {
                    event: "message".to_string(),
                    data: "first event".to_string(),
                    ..Default::default()
                },
                Event {
                    event: "message".to_string(),
                    data: "second event".to_string(),
                    ..Default::default()
                }
            ]
        );
    }

    #[test]
    fn test_one_complete_and_one_partial_event() {
        // First chunk contains one complete event and start of another
        let buf1 = String::from("data: complete\n\ndata: partial");
        let mut event_parser = EventParser::default();

        let events = event_parser
            .parse(buf1.into())
            .expect("should not return parsing error");

        // Should only parse the complete event
        assert_eq!(
            events,
            vec![Event {
                event: "message".to_string(),
                data: "complete".to_string(),
                ..Default::default()
            }]
        );

        let buf2 = String::from(" event\n\n");
        let events2 = event_parser
            .parse(buf2.into())
            .expect("should not return parsing error");

        assert_eq!(
            events2,
            vec![Event {
                event: "message".to_string(),
                data: "partial event".to_string(),
                ..Default::default()
            }]
        );
    }

    #[test]
    fn test_event_with_all_fields() {
        let buf = String::from("event: custom\ndata: test data\nid: 123\nretry: 5000\n\n");
        let mut event_parser = EventParser::default();

        let events = event_parser
            .parse(buf.into())
            .expect("should not return parsing error");
        assert_eq!(
            events,
            vec![Event {
                event: "custom".to_string(),
                data: "test data".to_string(),
                id: "123".to_string(),
                retry: Some(Duration::from_millis(5000)),
            }]
        );
    }

    #[test]
    fn test_event_with_multiple_data_lines() {
        let buf = String::from("data: line 1\ndata: line 2\ndata: line 3\n\n");
        let mut event_parser = EventParser::default();

        let events = event_parser
            .parse(buf.into())
            .expect("should not return parsing error");
        assert_eq!(
            events,
            vec![Event {
                event: "message".to_string(),
                data: "line 1\nline 2\nline 3".to_string(),
                ..Default::default()
            }]
        );
    }

    #[test]
    fn test_event_with_comments() {
        let buf = String::from(": this is a comment\ndata: actual data\n: another comment\n\n");
        let mut event_parser = EventParser::default();

        let events = event_parser
            .parse(buf.into())
            .expect("should not return parsing error");
        assert_eq!(
            events,
            vec![Event {
                event: "message".to_string(),
                data: "actual data".to_string(),
                ..Default::default()
            }]
        );
    }

    #[test]
    fn test_empty_data_no_event() {
        // Events with no data should not be dispatched
        let buf = String::from("event: test\nid: 123\n\n");
        let mut event_parser = EventParser::default();

        let events = event_parser
            .parse(buf.into())
            .expect("should not return parsing error");
        assert!(events.is_empty());
    }

    #[test]
    fn test_id_with_null_character_ignored() {
        // IDs containing null character should be ignored
        let buf = String::from("data: test\nid: invalid\u{0000}id\n\n");
        let mut event_parser = EventParser::default();

        let events = event_parser
            .parse(buf.into())
            .expect("should not return parsing error");
        assert_eq!(
            events,
            vec![Event {
                event: "message".to_string(),
                data: "test".to_string(),
                ..Default::default()
            }]
        );
    }

    #[test]
    fn test_invalid_retry_value() {
        // Invalid retry value should be ignored
        let buf = String::from("data: test\nretry: not_a_number\n\n");
        let mut event_parser = EventParser::default();

        let events = event_parser
            .parse(buf.into())
            .expect("should not return parsing error");
        assert_eq!(
            events,
            vec![Event {
                event: "message".to_string(),
                data: "test".to_string(),
                retry: None, // Should be None because value was invalid
                ..Default::default()
            }]
        );
    }

    #[test]
    fn test_data_with_trailing_lf() {
        // Data ending with LF should have it removed
        let buf = String::from("data: test data\n\n\n");
        let mut event_parser = EventParser::default();

        let events = event_parser
            .parse(buf.into())
            .expect("should not return parsing error");
        assert_eq!(
            events,
            vec![Event {
                event: "message".to_string(),
                data: "test data".to_string(),
                ..Default::default()
            }]
        );
    }

    #[test]
    fn test_field_without_value() {
        // Fields can have no value (colon only), which results in empty string value
        // However, events with empty data are not dispatched per SSE spec
        let buf = String::from("data:\n\n");
        let mut event_parser = EventParser::default();

        let events = event_parser
            .parse(buf.into())
            .expect("should not return parsing error");
        // Empty data events should not be dispatched
        assert!(events.is_empty());
    }

    #[test]
    fn test_partial_event_buffering() {
        // Test that partial events are properly buffered across multiple parse calls
        let buf1 = String::from("ev");
        let mut event_parser = EventParser::default();

        let events = event_parser
            .parse(buf1.into())
            .expect("should not return parsing error");
        assert!(events.is_empty());

        let buf2 = String::from("ent: test\ndata: some ");
        let events = event_parser
            .parse(buf2.into())
            .expect("should not return parsing error");
        assert!(events.is_empty());

        let buf3 = String::from("data\n\n");
        let events = event_parser
            .parse(buf3.into())
            .expect("should not return parsing error");
        assert_eq!(
            events,
            vec![Event {
                event: "test".to_string(),
                data: "some data".to_string(),
                ..Default::default()
            }]
        );
    }

    #[test]
    fn test_partial_event_data_buffering() {
        // Test that partial data events are properly buffered across multiple parse calls
        let buf1 = String::from("data: data1\n");
        let mut event_parser = EventParser::default();
        let events = event_parser
            .parse(buf1.into())
            .expect("should not return parsing error");
        assert!(events.is_empty());

        let buf2 = String::from("data: data2\n\n");
        let events = event_parser
            .parse(buf2.into())
            .expect("should not return parsing error");

        assert_eq!(
            events,
            vec![Event {
                event: "message".to_string(),
                data: "data1\ndata2".to_string(),
                ..Default::default()
            }]
        );
    }

    #[test]
    fn sse_body_parser_extracts_from_matching_event() {
        let mut parser = SseBodyParser::new(vec![group("/usage/total_tokens")]);

        let chunk = b"data: {\"usage\":{\"total_tokens\":42}}\n\ndata: [DONE]\n\n";
        parser.feed(chunk).expect("feed should succeed");
        parser.finalize().expect("finalize should succeed");

        assert!(parser.is_complete());
        assert!(parser.remaining_fields().is_empty());

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(
            body_ctx.get_value("/usage/total_tokens"),
            Some(&Value::Int(42))
        );
    }

    #[test]
    fn sse_number_hint_resolves_a_numeric_string() {
        // A `number`-hinted group must accept "150" the same way the
        // non-streaming path does, not just a literal JSON number.
        let field = BodyFieldGroup::new(
            vec!["/usage/total_tokens".to_string()],
            Some(ExpectedType::Number),
        );
        let mut parser = SseBodyParser::new(vec![field.clone()]);

        let chunk = b"data: {\"usage\":{\"total_tokens\":\"150\"}}\n\ndata: [DONE]\n\n";
        parser.feed(chunk).expect("feed should succeed");
        parser.finalize().expect("finalize should succeed");

        assert!(parser.remaining_fields().is_empty());

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(body_ctx.get_value(&field.key), Some(&Value::Int(150)));
    }

    #[test]
    fn sse_untyped_group_still_preserves_string_type() {
        // An untyped (or `string`-hinted) group must keep preserving the
        // actual JSON string, unaffected by the `number` hint's leniency.
        let field = BodyFieldGroup::new(vec!["/model".to_string()], None);
        let mut parser = SseBodyParser::new(vec![field.clone()]);

        let chunk = b"data: {\"model\":\"150\"}\n\ndata: [DONE]\n\n";
        parser.feed(chunk).expect("feed should succeed");
        parser.finalize().expect("finalize should succeed");

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(
            body_ctx.get_value(&field.key),
            Some(&Value::String(std::sync::Arc::new("150".to_string())))
        );
    }

    #[test]
    fn sse_body_parser_multiline_json_data() {
        let mut parser = SseBodyParser::new(vec![group("/usage/total_tokens")]);

        let chunk = b"data: {\"usage\":\ndata: {\"total_tokens\":42}}\n\ndata: [DONE]\n\n";
        parser.feed(chunk).expect("feed should succeed");
        parser.finalize().expect("finalize should succeed");

        assert!(parser.is_complete());

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(
            body_ctx.get_value("/usage/total_tokens"),
            Some(&Value::Int(42))
        );
    }

    #[test]
    fn sse_body_parser_multi_chunk() {
        let mut parser = SseBodyParser::new(vec![group("/usage/prompt_tokens")]);

        parser
            .feed(b"data: {\"id\":\"chunk1\"}\n\n")
            .expect("feed should succeed");
        parser
            .feed(b"data: {\"usage\":{\"prompt_tokens\":10}}\n\ndata: [DONE]\n\n")
            .expect("feed should succeed");
        parser.finalize().expect("finalize should succeed");

        assert!(parser.is_complete());

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(
            body_ctx.get_value("/usage/prompt_tokens"),
            Some(&Value::Int(10))
        );
    }

    #[test]
    fn sse_body_parser_missing_field() {
        let mut parser = SseBodyParser::new(vec![group("/nonexistent")]);

        let chunk = b"data: {\"usage\":{\"total_tokens\":42}}\n\ndata: [DONE]\n\n";
        parser.feed(chunk).expect("feed should succeed");
        parser.finalize().expect("finalize should succeed");

        assert!(parser.is_complete());
        let expected = group("/nonexistent");
        assert_eq!(parser.remaining_fields(), vec![&expected]);

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert!(body_ctx.get_value("/nonexistent").is_none());
    }

    #[test]
    fn sse_body_parser_only_done_event() {
        let mut parser = SseBodyParser::new(vec![group("/usage")]);

        let chunk = b"data: [DONE]\n\n";
        parser.feed(chunk).expect("feed should succeed");
        parser.finalize().expect("finalize should succeed");

        assert!(parser.is_complete());
        let expected = group("/usage");
        assert_eq!(parser.remaining_fields(), vec![&expected]);
    }

    #[test]
    fn sse_body_parser_non_matching_garbage_event_is_not_fatal() {
        // "not valid json" doesn't contain the leaf key `"field"` at all, so it's
        // never even attempted as JSON -- it's just discarded, same as any other
        // irrelevant event (a heartbeat, a comment, unrelated content).
        let mut parser = SseBodyParser::new(vec![group("/field")]);

        let chunk = b"data: not valid json\n\ndata: [DONE]\n\n";
        parser.feed(chunk).expect("feed should succeed");
        parser.finalize().expect("finalize should succeed");

        assert_eq!(parser.remaining_fields(), vec![&group("/field")]);
    }

    #[test]
    fn sse_body_parser_matching_but_malformed_event_is_skipped_not_fatal() {
        // This event's data does contain the leaf key `"field"`, so it *is*
        // attempted as JSON -- and fails. That must not fail the whole task: a
        // later event can still resolve the field.
        let mut parser = SseBodyParser::new(vec![group("/field")]);

        let chunk =
            b"data: {\"field\": this is not valid json}\n\ndata: {\"field\":42}\n\ndata: [DONE]\n\n";
        parser.feed(chunk).expect("feed should succeed");
        parser.finalize().expect("finalize should succeed");

        assert!(parser.remaining_fields().is_empty());

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(body_ctx.get_value("/field"), Some(&Value::Int(42)));
    }

    #[test]
    fn sse_body_parser_bytes_consumed_always_zero() {
        let mut parser = SseBodyParser::new(vec![group("/field")]);
        assert_eq!(parser.bytes_consumed(), 0);

        parser
            .feed(b"data: {\"field\":1}\n\n")
            .expect("feed should succeed");
        assert_eq!(parser.bytes_consumed(), 0);
    }

    #[test]
    fn sse_body_parser_multiple_fields() {
        let mut parser =
            SseBodyParser::new(groups(&["/usage/prompt_tokens", "/usage/total_tokens"]));

        let chunk =
            b"data: {\"usage\":{\"prompt_tokens\":10,\"total_tokens\":42}}\n\ndata: [DONE]\n\n";
        parser.feed(chunk).expect("feed should succeed");
        parser.finalize().expect("finalize should succeed");

        assert!(parser.is_complete());
        assert!(parser.remaining_fields().is_empty());

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(
            body_ctx.get_value("/usage/prompt_tokens"),
            Some(&Value::Int(10))
        );
        assert_eq!(
            body_ctx.get_value("/usage/total_tokens"),
            Some(&Value::Int(42))
        );
    }

    #[test]
    fn sse_body_parser_not_complete_before_finalize() {
        let mut parser = SseBodyParser::new(vec![group("/field")]);

        parser
            .feed(b"data: {\"field\":1}\n\ndata: [DONE]\n\n")
            .expect("feed should succeed");

        assert!(!parser.is_complete());

        parser.finalize().expect("finalize should succeed");
        assert!(parser.is_complete());
    }

    #[test]
    fn sse_body_parser_preserves_string_type() {
        let mut parser = SseBodyParser::new(groups(&["/model", "/count"]));

        let chunk = b"data: {\"model\":\"42\",\"count\":42}\n\ndata: [DONE]\n\n";
        parser.feed(chunk).expect("feed should succeed");
        parser.finalize().expect("finalize should succeed");

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(
            body_ctx.get_value("/model"),
            Some(&Value::String(std::sync::Arc::new("42".to_string())))
        );
        assert_eq!(body_ctx.get_value("/count"), Some(&Value::Int(42)));
    }

    // --- Provider-agnostic strategy: new coverage per RFC 0024 ---

    #[test]
    fn leaf_key_substring_does_not_collide_on_shared_suffix() {
        // The exact case RFC 0024 calls out: `input_tokens` is a suffix of
        // Anthropic's `cache_read_input_tokens`, but quote-delimiting must stop
        // the shorter key's substring from matching inside the longer one.
        let key = leaf_key_substring("/usage/input_tokens").unwrap();
        assert_eq!(key, "\"input_tokens\"");
        assert!(!"\"cache_read_input_tokens\"".contains(key.as_str()));
    }

    #[test]
    fn leaf_key_substring_none_for_array_index_leaf() {
        // A pointer ending in an array index has no textual object key to
        // scan for at all -- `leaf_key_substring` must say so rather than
        // returning a bogus, near-unmatchable `"0"` substring.
        assert_eq!(leaf_key_substring("/items/0"), None);
    }

    #[test]
    fn array_index_leaf_candidate_still_resolves_via_json_pointer() {
        // Regression for the pre-filter starving a candidate it can't prove
        // absent: a group whose only candidate ends in an array index used to
        // never be handed to `serde_json::from_str` at all (its leaf-key list
        // was empty, so `data.contains(..)` never matched), even though
        // `json.pointer()` fully supports array indices and would have
        // resolved it.
        let field = BodyFieldGroup::new(vec!["/items/0".to_string()], None);
        let mut parser = SseBodyParser::new(vec![field.clone()]);

        parser
            .feed(b"data: {\"items\":[42]}\n\ndata: [DONE]\n\n")
            .expect("feed should succeed");
        parser.finalize().expect("finalize should succeed");

        assert!(parser.remaining_fields().is_empty());
        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(body_ctx.get_value(&field.key), Some(&Value::Int(42)));
    }

    #[test]
    fn unrelated_key_sharing_a_suffix_does_not_cause_incorrect_resolution() {
        // End-to-end version of the above: an event containing only the
        // unrelated, longer key must not resolve the field; a later event with
        // the real key must.
        let field = BodyFieldGroup::new(vec!["/usage/input_tokens".to_string()], None);
        let mut parser = SseBodyParser::new(vec![field.clone()]);

        parser
            .feed(b"data: {\"usage\":{\"cache_read_input_tokens\":999}}\n\n")
            .expect("feed should succeed");
        assert_eq!(parser.remaining_fields(), vec![&field]);

        parser
            .feed(b"data: {\"usage\":{\"input_tokens\":25}}\n\ndata: [DONE]\n\n")
            .expect("feed should succeed");
        parser.finalize().expect("finalize should succeed");

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(
            body_ctx.get_value("/usage/input_tokens"),
            Some(&Value::Int(25))
        );
    }

    #[test]
    fn openai_responses_api_streaming_shape() {
        // OpenAI's Responses API nests the whole response object inside a named
        // `response.completed` event; the Chat Completions candidate is absent.
        let field = BodyFieldGroup::new(
            vec![
                "/usage/total_tokens".to_string(),
                "/response/usage/total_tokens".to_string(),
            ],
            Some(ExpectedType::Number),
        );
        let mut parser = SseBodyParser::new(vec![field.clone()]);

        let chunk = b"event: response.completed\ndata: {\"response\":{\"usage\":{\"total_tokens\":24}}}\n\n";
        parser.feed(chunk).expect("feed should succeed");
        parser.finalize().expect("finalize should succeed");

        assert!(parser.remaining_fields().is_empty());
        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(body_ctx.get_value(&field.key), Some(&Value::Int(24)));
    }

    #[test]
    fn anthropic_streaming_shape_no_terminal_sentinel() {
        // Usage split across message_start (input, nested under "message") and
        // message_delta (output, top-level) -- Anthropic's real shape -- with no
        // [DONE]-style sentinel anywhere. This is the exact case the old
        // "second-to-last event" heuristic could not handle in general.
        let input_field =
            BodyFieldGroup::new(vec!["/message/usage/input_tokens".to_string()], None);
        let output_field = BodyFieldGroup::new(vec!["/usage/output_tokens".to_string()], None);
        let mut parser = SseBodyParser::new(vec![input_field.clone(), output_field.clone()]);

        parser
            .feed(
                b"event: message_start\n\
                  data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":25,\"output_tokens\":0}}}\n\n\
                  event: content_block_delta\n\
                  data: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"hi\"}}\n\n\
                  event: message_delta\n\
                  data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":8}}\n\n\
                  event: message_stop\n\
                  data: {\"type\":\"message_stop\"}\n\n",
            )
            .expect("feed should succeed");
        parser.finalize().expect("finalize should succeed");

        assert!(parser.remaining_fields().is_empty());
        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(body_ctx.get_value(&input_field.key), Some(&Value::Int(25)));
        // message_start's nested output_tokens (0) lives at a different path
        // (`/message/usage/output_tokens`) and never matches this candidate;
        // only message_delta's top-level output_tokens does.
        assert_eq!(body_ctx.get_value(&output_field.key), Some(&Value::Int(8)));
    }

    #[test]
    fn gemini_streaming_shape_usage_only_in_true_final_chunk() {
        // Every chunk but the true last one omits usageMetadata entirely, and
        // there's no terminal sentinel -- the case the old heuristic (look at
        // the second-to-last event) got exactly wrong.
        let field = BodyFieldGroup::new(
            vec!["/usageMetadata/totalTokenCount".to_string()],
            Some(ExpectedType::Number),
        );
        let mut parser = SseBodyParser::new(vec![field.clone()]);

        parser
            .feed(
                b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"h\"}]}}]}\n\n\
                  data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"i\"}]}}]}\n\n\
                  data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"\"}],\"finishReason\":\"STOP\"}}],\"usageMetadata\":{\"totalTokenCount\":18}}\n\n",
            )
            .expect("feed should succeed");
        parser.finalize().expect("finalize should succeed");

        assert!(parser.remaining_fields().is_empty());
        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(body_ctx.get_value(&field.key), Some(&Value::Int(18)));
    }

    #[test]
    fn first_event_to_resolve_wins_even_though_it_is_lower_priority() {
        // The spec: "the first candidate that resolves (and matches the type
        // hint, if given) wins." This is *not* "highest priority wins" -- a
        // later event's higher-priority candidate must not override a value
        // an earlier event in the same `feed()` call already resolved via a
        // lower-priority one.
        let field = BodyFieldGroup::new(
            vec![
                "/usage/total_tokens".to_string(),
                "/usageMetadata/totalTokenCount".to_string(),
            ],
            None,
        );
        let mut parser = SseBodyParser::new(vec![field.clone()]);

        parser
            .feed(
                b"data: {\"usageMetadata\":{\"totalTokenCount\":99}}\n\n\
                  data: {\"usage\":{\"total_tokens\":42}}\n\n",
            )
            .expect("feed should succeed");
        parser.finalize().expect("finalize should succeed");

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(body_ctx.get_value(&field.key), Some(&Value::Int(99)));
    }

    #[test]
    fn first_resolved_value_survives_a_later_higher_priority_chunk() {
        // Same invariant as `first_event_to_resolve_wins_even_though_it_is_lower_priority`,
        // but across two separate `feed()` calls instead of two events within
        // one call -- proving the rule doesn't depend on how the stream
        // happens to be chunked. Cross-chunk analogue of JsonBodyParser's
        // `candidate_seen_first_in_the_stream_wins_over_higher_list_priority`.
        let field = BodyFieldGroup::new(
            vec![
                "/usage/total_tokens".to_string(),
                "/usageMetadata/totalTokenCount".to_string(),
            ],
            None,
        );
        let mut parser = SseBodyParser::new(vec![field.clone()]);

        parser
            .feed(b"data: {\"usageMetadata\":{\"totalTokenCount\":99}}\n\n")
            .expect("feed should succeed");
        parser
            .feed(b"data: {\"usage\":{\"total_tokens\":42}}\n\n")
            .expect("feed should succeed");
        parser.finalize().expect("finalize should succeed");

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(body_ctx.get_value(&field.key), Some(&Value::Int(99)));
    }

    #[test]
    fn both_candidates_present_in_same_event_highest_priority_wins() {
        // When a single event's JSON contains matches for more than one of a
        // field's candidates, they all resolve in the same pass (there's no
        // "earlier"/"later" event here), so ordinary list-order priority picks
        // the winner, regardless of which happens to appear first/last in the
        // JSON text.
        let field = BodyFieldGroup::new(
            vec![
                "/usage/total_tokens".to_string(),
                "/usageMetadata/totalTokenCount".to_string(),
            ],
            None,
        );
        let mut parser = SseBodyParser::new(vec![field.clone()]);

        parser
            .feed(b"data: {\"usage\":{\"total_tokens\":42},\"usageMetadata\":{\"totalTokenCount\":99}}\n\n")
            .expect("feed should succeed");
        parser.finalize().expect("finalize should succeed");

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(body_ctx.get_value(&field.key), Some(&Value::Int(42)));
    }

    #[test]
    fn same_candidate_repeated_across_events_keeps_first_value() {
        // No same-candidate progressive-update support: once a group
        // resolves, it's done, even if a later event re-reports the exact
        // same candidate with a different (e.g. more complete/cumulative)
        // value.
        let field = BodyFieldGroup::new(vec!["/usage/total_tokens".to_string()], None);
        let mut parser = SseBodyParser::new(vec![field.clone()]);

        parser
            .feed(b"data: {\"usage\":{\"total_tokens\":10}}\n\ndata: {\"usage\":{\"total_tokens\":24}}\n\n")
            .expect("feed should succeed");
        parser.finalize().expect("finalize should succeed");

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(body_ctx.get_value(&field.key), Some(&Value::Int(10)));
    }

    // --- Parity with JsonBodyParser's type-hint/fallthrough coverage ---

    #[test]
    fn sse_untyped_group_skips_a_null_candidate_and_falls_through() {
        // Mirrors JsonBodyParser's
        // `untyped_group_skips_a_null_candidate_and_falls_through`: no type
        // hint, so a present-but-null first candidate isn't "resolved"
        // either, and the group falls through to the next candidate.
        let field = BodyFieldGroup::new(
            vec![
                "/usage/total_tokens".to_string(),
                "/usageMetadata/totalTokenCount".to_string(),
            ],
            None,
        );
        let mut parser = SseBodyParser::new(vec![field.clone()]);

        parser
            .feed(b"data: {\"usage\":{\"total_tokens\":null},\"usageMetadata\":{\"totalTokenCount\":18}}\n\n")
            .expect("feed should succeed");
        parser.finalize().expect("finalize should succeed");

        assert!(parser.remaining_fields().is_empty());
        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(body_ctx.get_value(&field.key), Some(&Value::Int(18)));
    }

    #[test]
    fn sse_type_hint_skips_non_matching_candidate() {
        // Mirrors JsonBodyParser's `type_hint_skips_non_matching_candidate`:
        // the first-listed candidate resolves but isn't a number, so the
        // `number`-hinted group falls through to the second candidate.
        let field = BodyFieldGroup::new(
            vec!["/model".to_string(), "/usage/total_tokens".to_string()],
            Some(ExpectedType::Number),
        );
        let mut parser = SseBodyParser::new(vec![field.clone()]);

        parser
            .feed(b"data: {\"model\":\"gpt-4\",\"usage\":{\"total_tokens\":42}}\n\n")
            .expect("feed should succeed");
        parser.finalize().expect("finalize should succeed");

        assert!(parser.remaining_fields().is_empty());
        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(body_ctx.get_value(&field.key), Some(&Value::Int(42)));
    }

    #[test]
    fn sse_type_hint_with_no_matching_candidate_remains_unresolved() {
        // Mirrors JsonBodyParser's
        // `type_hint_with_no_matching_candidate_remains_unresolved`.
        let field = BodyFieldGroup::new(vec!["/model".to_string()], Some(ExpectedType::Number));
        let mut parser = SseBodyParser::new(vec![field.clone()]);

        parser
            .feed(b"data: {\"model\":\"gpt-4\"}\n\n")
            .expect("feed should succeed");
        parser.finalize().expect("finalize should succeed");

        assert_eq!(parser.remaining_fields(), vec![&field]);
    }

    #[test]
    fn sse_string_hint_rejects_a_bare_number() {
        // Mirrors JsonBodyParser's `string_hint_rejects_a_bare_number`: a
        // bare JSON number token is not a "string", even under a `string`
        // hint.
        let field = BodyFieldGroup::new(vec!["/x".to_string()], Some(ExpectedType::String));
        let mut parser = SseBodyParser::new(vec![field.clone()]);

        parser
            .feed(b"data: {\"x\":12345}\n\n")
            .expect("feed should succeed");
        parser.finalize().expect("finalize should succeed");

        assert_eq!(parser.remaining_fields(), vec![&field]);
    }

    #[test]
    fn sse_same_candidate_resolves_differently_for_groups_with_different_hints() {
        // Mirrors JsonBodyParser's
        // `same_candidate_resolves_differently_for_groups_with_different_hints`:
        // two groups sharing the same raw candidate, each with its own type
        // hint, both resolved correctly from the one JSON parse.
        let number_group = BodyFieldGroup::new(vec!["/x".to_string()], Some(ExpectedType::Number));
        let string_group = BodyFieldGroup::new(vec!["/x".to_string()], Some(ExpectedType::String));
        let mut parser = SseBodyParser::new(vec![number_group.clone(), string_group.clone()]);

        parser
            .feed(b"data: {\"x\":\"12345\"}\n\n")
            .expect("feed should succeed");
        parser.finalize().expect("finalize should succeed");

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(
            body_ctx.get_value(&number_group.key),
            Some(&Value::Int(12345))
        );
        assert_eq!(
            body_ctx.get_value(&string_group.key),
            Some(&Value::String(std::sync::Arc::new("12345".to_string())))
        );
    }
}
