use crate::data::attribute::AttributeState;
use crate::data::cel::PropSetter;
use crate::data::Headers;
use crate::kuadrant::pipeline::tasks::{Task, TaskOutcome};
use crate::kuadrant::ReqRespCtx;
use event_parser::Event;
use log::{error, warn};
use serde_json::Value;

mod event_parser;

pub struct TokenUsageTask {
    strategy: Option<Box<dyn ExtractionStrategy>>,
    prop_setter: PropSetter,
}

impl TokenUsageTask {
    #[cfg(test)]
    pub fn new() -> Self {
        Self::with_prop_setter(Default::default())
    }

    pub fn with_prop_setter(prop_setter: PropSetter) -> Self {
        Self {
            strategy: None,
            prop_setter,
        }
    }
}

impl From<Box<Self>> for TokenUsageTask {
    fn from(value: Box<Self>) -> Self {
        Self {
            strategy: value.strategy,
            prop_setter: value.prop_setter,
        }
    }
}

impl Task for TokenUsageTask {
    fn pauses_filter(&self, ctx: &ReqRespCtx) -> bool {
        self.strategy.is_some() && !ctx.is_end_of_stream()
    }

    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        let mut task: TokenUsageTask = self.into();

        let mut strategy = match task.strategy.take() {
            Some(s) => s,
            None => {
                match ctx.get_attribute_ref::<Headers>(&"response.headers".into()) {
                    Ok(AttributeState::Available(Some(headers))) => select_strategy(&headers),
                    Ok(AttributeState::Available(None)) => {
                        unreachable!("Headers map should never be Available(None)")
                    }
                    Ok(AttributeState::Pending) => {
                        // Headers not available yet, requeue
                        return TaskOutcome::Requeued(vec![Box::new(task)]);
                    }
                    Err(e) => {
                        error!("Failed to get response headers: {e:?}");
                        return TaskOutcome::Failed;
                    }
                }
            }
        };

        if ctx.response_body_buffer_size() > 0 {
            match ctx.get_http_response_body(0, ctx.response_body_buffer_size()) {
                Ok(AttributeState::Available(Some(bytes))) => {
                    strategy.feed_buffer(bytes);
                }
                Ok(AttributeState::Available(None)) => {
                    // todo(refactor): No bytes available?
                }
                Ok(AttributeState::Pending) => {
                    task.strategy = Some(strategy);
                    return TaskOutcome::Requeued(vec![Box::new(task)]);
                }
                Err(e) => {
                    error!("Failed to get response body: {e:?}");
                    return TaskOutcome::Failed;
                }
            }
        }

        if ctx.is_end_of_stream() {
            let props: Vec<String> = task.prop_setter.expected_props().to_vec();

            for prop in props {
                if let Some(json_value) = strategy.extract_property(&prop) {
                    match json_value {
                        Value::Bool(b) => task.prop_setter.set_prop(prop, b),
                        Value::Number(n) => {
                            if let Some(u) = n.as_u64() {
                                task.prop_setter.set_prop(prop, u);
                            } else if let Some(i) = n.as_i64() {
                                task.prop_setter.set_prop(prop, i);
                            } else if let Some(f) = n.as_f64() {
                                task.prop_setter.set_prop(prop, f);
                            }
                        }
                        Value::String(s) => task.prop_setter.set_prop(prop, s),
                        // todo(refactor): unimplemented?
                        Value::Null | Value::Array(_) | Value::Object(_) => {}
                    }
                } else {
                    // todo(refactor): what do we do if property is missing?
                    warn!("Missing json property: {}", prop);
                }
            }
            return TaskOutcome::Done;
        }

        task.strategy = Some(strategy);
        TaskOutcome::Requeued(vec![Box::new(task)])
    }
}

fn select_strategy(headers: &Headers) -> Box<dyn ExtractionStrategy> {
    if let Some(ct) = headers.get("content-type") {
        if ct.contains("text/event-stream") {
            Box::new(SseStrategy::new())
        } else {
            Box::new(JsonStrategy::new())
        }
    } else {
        // default to JSON
        Box::new(JsonStrategy::new())
    }
}

trait ExtractionStrategy {
    fn feed_buffer(&mut self, bytes: Vec<u8>);
    fn extract_property(&mut self, prop: &str) -> Option<Value>;
}

struct SseStrategy {
    event_parser: event_parser::EventParser,
    // Stores the last two events: [second_to_last, last]
    last_two_events: [Option<Event>; 2],
    parsed_json: Option<Value>,
}

impl SseStrategy {
    fn new() -> Self {
        Self {
            event_parser: event_parser::EventParser::default(),
            last_two_events: [None, None],
            parsed_json: None,
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

    fn extract_property(&mut self, prop: &str) -> Option<Value> {
        if self.parsed_json.is_none() {
            if let Some(event) = &self.last_two_events[1] {
                if let Ok(json) = serde_json::from_str::<Value>(&event.data) {
                    self.parsed_json = Some(json);
                }
            }
        }
        self.parsed_json.as_ref()?.pointer(prop).cloned()
    }
}

struct JsonStrategy {
    buffer: Vec<u8>,
    parsed_json: Option<Value>,
}

impl JsonStrategy {
    fn new() -> Self {
        Self {
            buffer: Default::default(),
            parsed_json: None,
        }
    }
}

impl ExtractionStrategy for JsonStrategy {
    fn feed_buffer(&mut self, mut bytes: Vec<u8>) {
        self.buffer.append(&mut bytes);
    }

    fn extract_property(&mut self, prop: &str) -> Option<Value> {
        if self.parsed_json.is_none() {
            if let Ok(json_str) = String::from_utf8(self.buffer.clone()) {
                if let Ok(json) = serde_json::from_str::<Value>(&json_str) {
                    self.parsed_json = Some(json);
                }
            }
        }
        self.parsed_json.as_ref()?.pointer(prop).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kuadrant::MockWasmHost;
    use std::sync::Arc;

    #[test]
    fn test_no_data_and_not_end_of_stream() {
        let headers = vec![("content-type".to_string(), "application/json".to_string())];
        let mock_backend = MockWasmHost::new().with_map("response.headers".to_string(), headers);
        let mut ctx = ReqRespCtx::new(Arc::new(mock_backend));
        ctx.set_current_response_body_buffer_size(0, false);

        let task = Box::new(TokenUsageTask::new());

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Requeued(_)));
    }

    #[test]
    fn test_no_data_and_end_of_stream() {
        let headers = vec![("content-type".to_string(), "application/json".to_string())];
        let mock_backend = MockWasmHost::new().with_map("response.headers".to_string(), headers);
        let mut ctx = ReqRespCtx::new(Arc::new(mock_backend));
        ctx.set_current_response_body_buffer_size(0, true);

        let task = Box::new(TokenUsageTask::new());

        let outcome = task.apply(&mut ctx);
        // No paths to extract, empty result is ok
        assert!(matches!(outcome, TaskOutcome::Done));
    }

    #[test]
    fn test_sse_strategy_extracts_from_second_to_last_event() {
        let mut strategy = SseStrategy::new();

        let events = b"data: {\"usage\":{\"total_tokens\":10}}\n\ndata: [DONE]\n\n";
        strategy.feed_buffer(events.to_vec());

        let result = strategy.extract_property("/usage/total_tokens");

        assert!(result.is_some());
        assert_eq!(result.unwrap().as_u64(), Some(10));
    }

    #[test]
    fn test_sse_strategy_fails_with_only_one_event() {
        let mut strategy = SseStrategy::new();

        let events = b"data: [DONE]\n\n";
        strategy.feed_buffer(events.to_vec());

        let result = strategy.extract_property("/usage/total_tokens");

        assert!(result.is_none());
    }

    #[test]
    fn test_json_strategy_extracts_from_complete_body() {
        let mut strategy = JsonStrategy::new();

        let json = br#"{"usage":{"total_tokens":42,"prompt_tokens":10}}"#;
        strategy.feed_buffer(json.to_vec());

        let total_tokens = strategy.extract_property("/usage/total_tokens");
        let prompt_tokens = strategy.extract_property("/usage/prompt_tokens");

        assert!(total_tokens.is_some());
        assert_eq!(total_tokens.unwrap().as_u64(), Some(42));

        assert!(prompt_tokens.is_some());
        assert_eq!(prompt_tokens.unwrap().as_u64(), Some(10));
    }

    #[test]
    fn test_json_strategy_handles_missing_properties() {
        let mut strategy = JsonStrategy::new();

        let json = br#"{"usage":{"total_tokens":42}}"#;
        strategy.feed_buffer(json.to_vec());

        let total_tokens = strategy.extract_property("/usage/total_tokens");
        let nonexistent = strategy.extract_property("/usage/nonexistent");

        assert!(total_tokens.is_some());
        assert_eq!(total_tokens.unwrap().as_u64(), Some(42));

        assert!(nonexistent.is_none());
    }
}
