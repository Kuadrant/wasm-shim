use crate::data::attribute::AttributeState;
use crate::data::cel::PropSetter;
use crate::data::Headers;
use crate::kuadrant::pipeline::tasks::{Task, TaskOutcome};
use crate::kuadrant::ReqRespCtx;
use event_parser::Event;
use log::error;
use serde_json::Value;
use std::collections::HashMap;

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
            let paths: Vec<String> = task.prop_setter.expected_props().to_vec();
            let extracted = strategy.extract_properties(&paths);

            // todo(refactor): store the extracted properties in ctx
            for prop in paths.clone() {
                task.prop_setter.set_prop(prop, true);
            }
            if extracted.is_empty() && !paths.is_empty() {
                // todo(refactor): is this an expected failure?
                return TaskOutcome::Failed;
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

    fn extract_properties(self: Box<Self>, paths: &[String]) -> HashMap<String, Value>;
}

struct SseStrategy {
    event_parser: event_parser::EventParser,
    // Stores the last two events: [second_to_last, last]
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

        let paths = vec!["/usage/total_tokens".to_string()];
        let result = Box::new(strategy).extract_properties(&paths);

        assert_eq!(result.len(), 1);
        assert_eq!(
            result.get("/usage/total_tokens").unwrap().as_u64(),
            Some(10)
        );
    }

    #[test]
    fn test_sse_strategy_fails_with_only_one_event() {
        let mut strategy = SseStrategy::new();

        let events = b"data: [DONE]\n\n";
        strategy.feed_buffer(events.to_vec());

        let paths = vec!["/usage/total_tokens".to_string()];
        let result = Box::new(strategy).extract_properties(&paths);

        assert!(result.is_empty());
    }

    #[test]
    fn test_json_strategy_extracts_from_complete_body() {
        let mut strategy = JsonStrategy::new();

        let json = br#"{"usage":{"total_tokens":42,"prompt_tokens":10}}"#;
        strategy.feed_buffer(json.to_vec());

        let paths = vec![
            "/usage/total_tokens".to_string(),
            "/usage/prompt_tokens".to_string(),
        ];
        let result = Box::new(strategy).extract_properties(&paths);

        assert_eq!(result.len(), 2);
        assert_eq!(
            result.get("/usage/total_tokens").unwrap().as_u64(),
            Some(42)
        );
        assert_eq!(
            result.get("/usage/prompt_tokens").unwrap().as_u64(),
            Some(10)
        );
    }

    #[test]
    fn test_json_strategy_handles_missing_properties() {
        let mut strategy = JsonStrategy::new();

        let json = br#"{"usage":{"total_tokens":42}}"#;
        strategy.feed_buffer(json.to_vec());

        let paths = vec![
            "/usage/total_tokens".to_string(),
            "/usage/nonexistent".to_string(),
        ];
        let result = Box::new(strategy).extract_properties(&paths);

        assert_eq!(result.len(), 1);
        assert_eq!(
            result.get("/usage/total_tokens").unwrap().as_u64(),
            Some(42)
        );
        assert!(result.get("/usage/nonexistent").is_none());
    }

    #[test]
    fn test_task_fails_when_paths_expected_but_no_data() {
        let headers = vec![("content-type".to_string(), "application/json".to_string())];
        let mock_backend = MockWasmHost::new().with_map("response.headers".to_string(), headers);
        let mut ctx = ReqRespCtx::new(Arc::new(mock_backend));
        ctx.set_current_response_body_buffer_size(0, true);

        // Create a predicate that uses responseBodyJSON to set expected props
        use crate::data::cel::Predicate;
        let predicate = Predicate::new("responseBodyJSON('/usage/total_tokens') == 10").unwrap();
        let prop_setter = PropSetter::new(&[predicate], &[]);
        let task = Box::new(TokenUsageTask::with_prop_setter(prop_setter));

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Failed));
    }
}
