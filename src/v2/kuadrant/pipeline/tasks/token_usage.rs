use crate::v2::kuadrant::pipeline::tasks::{Task, TaskOutcome};
use crate::v2::kuadrant::ReqRespCtx;

mod event_builder;
mod sse_parser;

pub struct TokenUsageTask {
    event_builder: event_builder::EventBuilder,
}

impl TokenUsageTask {
    pub fn new() -> Self {
        Self {
            event_builder: event_builder::EventBuilder::new(),
        }
    }
}

impl Task for TokenUsageTask {
    fn apply(self: Box<Self>, _ctx: &mut ReqRespCtx) -> TaskOutcome {
        // Extract token usage from the second-to-last Server-Sent Event.
        //
        // OpenAI streaming responses typically end with a [DONE] marker, meaning the usage data
        // appears in the event immediately before it. By targeting only this event, we
        // avoid parsing the entire stream.
        //
        // Example:
        //   Second-to-last: data: {"id":"...","usage":{"prompt_tokens":0,"completion_tokens":4,"total_tokens":4},...}
        //   Last:           data: [DONE]

        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::kuadrant::MockWasmHost;
    use std::sync::Arc;

    #[test]
    fn test_no_data_and_not_end_of_stream() {
        let mock_backend = MockWasmHost::new();
        let mut ctx = ReqRespCtx::new(Arc::new(mock_backend)).with_end_of_stream(false);

        let task = Box::new(TokenUsageTask::new());

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Requeued(_)));
    }

    #[test]
    fn test_no_data_and_end_of_stream() {
        let mock_backend = MockWasmHost::new();
        let mut ctx = ReqRespCtx::new(Arc::new(mock_backend)).with_end_of_stream(true);

        let task = Box::new(TokenUsageTask::new());

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Failed));
    }

    #[test]
    fn test_one_message_and_end_of_stream() {
        let buf = String::from(r#"data: {"id": 1}\n\n"#);
        let mock_backend = MockWasmHost::new().with_response_body(buf.as_bytes());
        let mut ctx = ReqRespCtx::new(Arc::new(mock_backend))
            .with_end_of_stream(true)
            .with_body_size(buf.len());

        let task = Box::new(TokenUsageTask::new());

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Failed));
    }

    #[test]
    fn test_no_usage_and_end_of_stream() {
        let buf = String::from(r#"data: {"id": 1}\n\ndata: {"id": 2}\n\n"#);
        let mock_backend = MockWasmHost::new().with_response_body(buf.as_bytes());
        let mut ctx = ReqRespCtx::new(Arc::new(mock_backend))
            .with_end_of_stream(true)
            .with_body_size(buf.len());

        let task = Box::new(TokenUsageTask::new());

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Failed));
    }

    #[test]
    fn test_no_usage_and_not_end_of_stream() {
        let buf = String::from(r#"data: {"id": 1}\n\ndata: {"id": 2}\n\n"#);
        let mock_backend = MockWasmHost::new().with_response_body(buf.as_bytes());
        let mut ctx = ReqRespCtx::new(Arc::new(mock_backend))
            .with_end_of_stream(false)
            .with_body_size(buf.len());

        let task = Box::new(TokenUsageTask::new());

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Requeued(_)));
    }

    #[test]
    fn test_usage_and_not_end_of_stream() {
        let buf = String::from(
            r#"data: {"id": 1}\n\ndata: {"id": 2, "usage":{"prompt_tokens":0,"completion_tokens":4,"total_tokens":4}}\n\n"#,
        );
        let mock_backend = MockWasmHost::new().with_response_body(buf.as_bytes());
        let mut ctx = ReqRespCtx::new(Arc::new(mock_backend))
            .with_end_of_stream(false)
            .with_body_size(buf.len());

        let task = Box::new(TokenUsageTask::new());

        let outcome = task.apply(&mut ctx);
        // the task will not parse until end_of_stream is signaled
        assert!(matches!(outcome, TaskOutcome::Requeued(_)));
    }

    #[test]
    fn test_usage_and_end_of_stream() {
        let buf = String::from(
            r#"data: {"id": 1}\n\ndata: {"id": 2, "usage":{"prompt_tokens":0,"completion_tokens":4,"total_tokens":4}}\n\ndata: [DONE]\n\n"#,
        );
        let mock_backend = MockWasmHost::new().with_response_body(buf.as_bytes());
        let mut ctx = ReqRespCtx::new(Arc::new(mock_backend))
            .with_end_of_stream(true)
            .with_body_size(buf.len());

        let task = Box::new(TokenUsageTask::new());

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Done));
        // assert on changes made to the ReqRespCtx (like adding the attribute with the usage
        // value)
        //
        let buf = String::from(
            r#"data: {"id": 1, "usage":{"prompt_tokens":0,"completion_tokens":4,"total_tokens":4}}\n\ndata: {"id": 2}\n\n"#,
        );
        let mock_backend = MockWasmHost::new().with_response_body(buf.as_bytes());
        let mut ctx = ReqRespCtx::new(Arc::new(mock_backend))
            .with_end_of_stream(true)
            .with_body_size(buf.len());

        let task = Box::new(TokenUsageTask::new());

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Done));
        // assert on changes made to the ReqRespCtx (like adding the attribute with the usage
        // value)
    }

    #[test]
    fn test_usage_not_in_second_to_last() {
        let buf = String::from(
            r#"data: {"id": 1}\n\ndata: {"id": 2, "usage":{"prompt_tokens":0,"completion_tokens":4,"total_tokens":4}}\n\n"#,
        );
        let mock_backend = MockWasmHost::new().with_response_body(buf.as_bytes());
        let mut ctx = ReqRespCtx::new(Arc::new(mock_backend))
            .with_end_of_stream(true)
            .with_body_size(buf.len());

        let task = Box::new(TokenUsageTask::new());

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Failed));
    }
}
