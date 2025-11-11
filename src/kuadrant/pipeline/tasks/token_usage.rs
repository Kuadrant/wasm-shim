use crate::data::attribute::AttributeState;
use crate::data::cel::PropSetter;
use crate::kuadrant::pipeline::tasks::{Task, TaskOutcome};
use crate::kuadrant::ReqRespCtx;
use event_parser::Event;
use serde_json::Value;
use std::collections::HashMap;

mod event_parser;

pub struct TokenUsageTask {
    event_parser: event_parser::EventParser,
    // Stores the last two events: [second_to_last, last]
    last_two_events: [Option<Event>; 2],
    started: bool,
    prop_setter: PropSetter,
}

impl TokenUsageTask {
    #[cfg(test)]
    pub fn new() -> Self {
        Self::with_prop_setter(Default::default())
    }

    pub fn with_prop_setter(prop_setter: PropSetter) -> Self {
        Self {
            event_parser: event_parser::EventParser::default(),
            last_two_events: [None, None],
            started: false,
            prop_setter,
        }
    }

    fn push_event(&mut self, event: Event) {
        // Shift the event at position 0 to position 1 (discarding old position 1)
        self.last_two_events[1] = self.last_two_events[0].take();
        // Insert the new event at position 0
        self.last_two_events[0] = Some(event);
    }
}

impl From<Box<Self>> for TokenUsageTask {
    fn from(value: Box<Self>) -> Self {
        Self {
            event_parser: value.event_parser,
            last_two_events: value.last_two_events,
            started: value.started,
            prop_setter: value.prop_setter,
        }
    }
}

impl Task for TokenUsageTask {
    fn pauses_filter(&self, ctx: &ReqRespCtx) -> bool {
        self.started && !ctx.is_end_of_stream()
    }

    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        // Extract token usage from the second-to-last Server-Sent Event.
        //
        // OpenAI streaming responses typically end with a [DONE] marker, meaning the usage data
        // appears in the event immediately before it. By targeting only this event, we don't need
        // to store all events, but are still parsing the entire stream.
        //
        // Example:
        //   Second-to-last: data: {"id":"...","usage":{"prompt_tokens":0,"completion_tokens":4,"total_tokens":4},...}
        //   Last:           data: [DONE]

        // TODO: check response content type is text/event-stream

        let mut new_t: TokenUsageTask = self.into();

        if ctx.response_body_buffer_size() == 0 && !ctx.is_end_of_stream(){
            return TaskOutcome::Requeued(vec![Box::new(new_t)]);
        }

        let chunk_bytes = match ctx.get_http_response_body(0, ctx.response_body_buffer_size()) {
            Ok(AttributeState::Available(bytes)) => bytes.unwrap_or_default(),
            Ok(AttributeState::Pending) => {
                return TaskOutcome::Requeued(vec![Box::new(new_t)]);
            }
            Err(_err) => return TaskOutcome::Failed,
        };

        new_t.started = true;

        match new_t.event_parser.parse(chunk_bytes) {
            Ok(events) => {
                for event in events {
                    new_t.push_event(event);
                }
            }
            Err(_e) => {
                // TODO: propagate the error with the Failed outcome
                return TaskOutcome::Failed;
            }
        }

        match (ctx.is_end_of_stream(), &new_t.last_two_events[1]) {
            // TODO: probably good to add some error
            // message saying not enough events where parsed
            (true, None) => TaskOutcome::Failed,
            (true, Some(_event)) => {
                // TODO: parse the event for the props!
                let props: Vec<String> = new_t.prop_setter.expected_props().to_vec();
                for prop in props {
                    new_t.prop_setter.set_prop(prop, true);
                }
                TaskOutcome::Done
            }
            (false, _) => TaskOutcome::Requeued(vec![Box::new(new_t)]),
        }
    }
}

trait ExtractionStrategy {
    fn feed_buffer(&mut self, bytes: Vec<u8>);

    fn extract_properties(self: Box<Self>, paths: &[String]) -> HashMap<String, Value>;
}

struct SseStrategy {
    event_parser: event_parser::EventParser,
    last_two_events: [Option<Event>; 2],
}

impl SseStrategy {
    fn new() -> Self {
        Self {
            event_parser: event_parser::EventParser::default(),
            last_two_events: [None, None],
        }
    }

    fn push_event(&mut self, event: Event) {
        self.last_two_events[1] = self.last_two_events[0].take();
        self.last_two_events[0] = Some(event);
    }
}

impl ExtractionStrategy for SseStrategy {
    fn feed_buffer(&mut self, bytes: Vec<u8>) {
        if let Ok(events) = self.event_parser.parse(bytes) {
            for event in events {
                self.push_event(event);
            }
        }
    }

    fn extract_properties(self: Box<Self>, paths: &[String]) -> HashMap<String, Value> {
        if let Some(event) = &self.last_two_events[1] {
            if let Ok(json) = serde_json::from_str::<Value>(&event.data) {
                return extract_properties_from_json(&json, paths);
            }
        }
        HashMap::new()
    }
}

struct JsonStrategy {
    buffer: Vec<u8>,
}

impl JsonStrategy {
    fn new() -> Self {
        Self {
            buffer: Default::default(),
        }
    }
}

impl ExtractionStrategy for JsonStrategy {
    fn feed_buffer(&mut self, mut bytes: Vec<u8>) {
        self.buffer.append(&mut bytes);
    }

    fn extract_properties(self: Box<Self>, paths: &[String]) -> HashMap<String, Value> {
        if let Ok(json_str) = String::from_utf8(self.buffer) {
            if let Ok(json) = serde_json::from_str::<Value>(&json_str) {
                return extract_properties_from_json(&json, paths);
            }
        }
        HashMap::new()
    }
}

fn extract_properties_from_json(json: &Value, paths: &[String]) -> HashMap<String, Value> {
    let mut result = HashMap::new();

    for path in paths {
        if let Some(value) = json.pointer(path) {
            result.insert(path.clone(), value.clone());
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kuadrant::MockWasmHost;
    use std::sync::Arc;

    #[test]
    fn test_no_data_and_not_end_of_stream() {
        let mock_backend = MockWasmHost::new();
        let mut ctx = ReqRespCtx::new(Arc::new(mock_backend));
        ctx.set_current_response_body_buffer_size(0, false);

        let task = Box::new(TokenUsageTask::new());

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Requeued(_)));
    }

    #[test]
    fn test_no_data_and_end_of_stream() {
        let mock_backend = MockWasmHost::new();
        let mut ctx = ReqRespCtx::new(Arc::new(mock_backend));
        ctx.set_current_response_body_buffer_size(0, true);

        let task = Box::new(TokenUsageTask::new());

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Failed));
    }

    #[test]
    fn test_one_event_and_end_of_stream() {
        let buf = String::from("data:foo\n\n");
        let mock_backend = MockWasmHost::new().with_response_body(buf.as_bytes());
        let mut ctx = ReqRespCtx::new(Arc::new(mock_backend));
        ctx.set_current_response_body_buffer_size(buf.len(), true);

        let task = Box::new(TokenUsageTask::new());

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Failed));
    }

    #[test]
    fn test_two_events_and_not_end_of_stream() {
        let buf = String::from("data:foo\n\ndata:bar\n\n");
        let mock_backend = MockWasmHost::new().with_response_body(buf.as_bytes());
        let mut ctx = ReqRespCtx::new(Arc::new(mock_backend));
        ctx.set_current_response_body_buffer_size(buf.len(), false);

        let task = Box::new(TokenUsageTask::new());

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Requeued(_)));
    }

    #[test]
    fn test_two_events_and_end_of_stream() {
        let buf = String::from("data:foo\n\ndata:bar\n\n");
        let mock_backend = MockWasmHost::new().with_response_body(buf.as_bytes());
        let mut ctx = ReqRespCtx::new(Arc::new(mock_backend));
        ctx.set_current_response_body_buffer_size(buf.len(), true);

        let task = Box::new(TokenUsageTask::new());

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Done));
        // assert on changes made to the ReqRespCtx
        // like adding the second last event value
    }
}
