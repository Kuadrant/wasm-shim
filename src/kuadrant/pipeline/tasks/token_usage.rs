use crate::data::attribute::AttributeState;
use crate::kuadrant::pipeline::tasks::{Task, TaskOutcome};
use crate::kuadrant::ReqRespCtx;
use event_parser::Event;

mod event_parser;

pub struct TokenUsageTask {
    event_parser: event_parser::EventParser,
    // Stores the last two events: [second_to_last, last]
    last_two_events: [Option<Event>; 2],
    started: bool,
}

impl TokenUsageTask {
    pub fn new() -> Self {
        Self {
            event_parser: event_parser::EventParser::default(),
            last_two_events: [None, None],
            started: false,
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

        let chunk_bytes = match ctx.get_http_response_body(0, ctx.body_size()) {
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
                // TODO: store the event somewhere in the ctx?
                TaskOutcome::Done
            }
            (false, _) => TaskOutcome::Requeued(vec![Box::new(new_t)]),
        }
    }
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
