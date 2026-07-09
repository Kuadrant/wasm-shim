use std::collections::HashMap;

use cel::Value;
use core::time::Duration;
use sse_line_parser::RawEventLine;
use tracing::error;

use super::body_parser::{parse_json_scalar, BodyParser};
use crate::data::attribute::AttributeError;
use crate::kuadrant::context::BodyContext;

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

pub(crate) struct SseBodyParser {
    fields: Vec<String>,
    event_parser: EventParser,
    last_two_events: [Option<Event>; 2],
    extracted: HashMap<String, Value>,
    complete: bool,
}

impl SseBodyParser {
    pub fn new(fields: Vec<String>) -> Self {
        Self {
            fields,
            event_parser: EventParser::default(),
            last_two_events: [None, None],
            extracted: HashMap::new(),
            complete: false,
        }
    }

    fn push_event(&mut self, event: Event) {
        self.last_two_events[1] = self.last_two_events[0].take();
        self.last_two_events[0] = Some(event);
    }
}

impl BodyParser for SseBodyParser {
    fn bytes_consumed(&self) -> usize {
        0
    }

    fn finalize(&mut self) {
        self.complete = true;
        let penultimate = match &self.last_two_events[1] {
            Some(event) => &event.data,
            None => return,
        };

        let json: serde_json::Value = match serde_json::from_str(penultimate) {
            Ok(v) => v,
            Err(e) => {
                error!("Failed to parse penultimate SSE event as JSON: {e}");
                return;
            }
        };

        for field in &self.fields {
            if let Some(value) = json.pointer(field) {
                let cel_value = match value {
                    serde_json::Value::String(s) => Value::String(std::sync::Arc::new(s.clone())),
                    other => parse_json_scalar(&other.to_string()),
                };
                self.extracted.insert(field.clone(), cel_value);
            }
        }
    }

    fn is_complete(&self) -> bool {
        self.complete && self.remaining_fields().is_empty()
    }

    fn remaining_fields(&self) -> Vec<&String> {
        self.fields
            .iter()
            .filter(|f| !self.extracted.contains_key(f.as_str()))
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

        for event in events {
            self.push_event(event);
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
}
