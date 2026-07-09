mod body_parser;
mod json_body_parser;
pub(super) mod sse_body_parser;

use body_parser::BodyParser;
use json_body_parser::JsonBodyParser;
use sse_body_parser::SseBodyParser;
use tracing::error;

use crate::data::attribute::{AttributeError, AttributeState};
use crate::data::cel::Predicate;
use crate::data::Expression;
use crate::data::Headers;
use crate::kuadrant::pipeline::tasks::{SendReplyTask, Task, TaskOutcome};
use crate::kuadrant::{PathReservation, ReqRespCtx};
use crate::services::MessageConverter;

#[derive(Clone, Copy)]
enum BodySource {
    Request,
    Response,
}

enum BodyParseState {
    NotNeeded,
    Pending(BodySource, Vec<String>),
    Active(BodySource, Box<dyn BodyParser>),
}

pub struct StoreTask {
    task_id: String,
    predicate: Option<Predicate>,
    expression: Expression,
    path: String,
    export_to_host: bool,
    terminal: bool,
    body_parse_state: BodyParseState,
    _reservation: PathReservation,
}

impl StoreTask {
    pub fn new(
        ctx: &ReqRespCtx,
        task_id: String,
        predicate: Predicate,
        expression: Expression,
        path: String,
        export_to_host: bool,
        terminal: bool,
    ) -> Result<Self, AttributeError> {
        let body_parse_state = Self::initial_body_state(&predicate, &expression);
        let reservation = ctx.values.reserve(path.clone());
        Ok(Self {
            task_id,
            predicate: Some(predicate),
            expression,
            path,
            export_to_host,
            terminal,
            body_parse_state,
            _reservation: reservation,
        })
    }

    fn initial_body_state(predicate: &Predicate, expression: &Expression) -> BodyParseState {
        let mut request_fields: Vec<String> = Vec::new();
        let mut response_fields: Vec<String> = Vec::new();

        request_fields.extend_from_slice(predicate.request_body_values());
        request_fields.extend_from_slice(expression.request_body_values());

        response_fields.extend_from_slice(predicate.response_body_values());
        response_fields.extend_from_slice(expression.response_body_values());

        if !request_fields.is_empty() {
            request_fields.sort();
            request_fields.dedup();
            BodyParseState::Pending(BodySource::Request, request_fields)
        } else if !response_fields.is_empty() {
            response_fields.sort();
            response_fields.dedup();
            BodyParseState::Pending(BodySource::Response, response_fields)
        } else {
            BodyParseState::NotNeeded
        }
    }

    fn create_parser(
        ctx: &ReqRespCtx,
        source: BodySource,
        fields: &[String],
    ) -> Result<Option<Box<dyn BodyParser>>, AttributeError> {
        match source {
            BodySource::Request => {
                let parser = JsonBodyParser::new(fields.to_vec()).map_err(|e| {
                    AttributeError::Parse(format!("Failed to create request body parser: {e}"))
                })?;
                Ok(Some(Box::new(parser)))
            }
            BodySource::Response => {
                let is_sse = match ctx.get_attribute_ref::<Headers>(&"response.headers".into()) {
                    Ok(AttributeState::Available(Some(headers))) => headers
                        .get("content-type")
                        .is_some_and(|ct| ct.contains("text/event-stream")),
                    Ok(AttributeState::Available(None)) => false,
                    Ok(AttributeState::Pending) => return Ok(None),
                    Err(e) => {
                        error!("Failed to get response headers: {e:?}");
                        false
                    }
                };

                if is_sse {
                    Ok(Some(Box::new(SseBodyParser::new(fields.to_vec()))))
                } else {
                    let parser = JsonBodyParser::new(fields.to_vec()).map_err(|e| {
                        AttributeError::Parse(format!("Failed to create response body parser: {e}"))
                    })?;
                    Ok(Some(Box::new(parser)))
                }
            }
        }
    }
}

impl Task for StoreTask {
    fn id(&self) -> &str {
        &self.task_id
    }

    fn apply(mut self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        if let BodyParseState::Pending(source, ref fields) = self.body_parse_state {
            let result = Self::create_parser(ctx, source, fields);
            match result {
                Ok(Some(parser)) => {
                    self.body_parse_state = BodyParseState::Active(source, parser);
                }
                Ok(None) => {
                    return TaskOutcome::Requeued(vec![self]);
                }
                Err(e) => {
                    error!("Failed to create body parser: {e}");
                    return TaskOutcome::Failed;
                }
            }
        }

        if let BodyParseState::Active(source, ref mut parser) = self.body_parse_state {
            let body_ctx = match source {
                BodySource::Request => &ctx.request_body,
                BodySource::Response => &ctx.response_body,
            };

            if body_ctx.buffer_size() == 0 && !body_ctx.is_end_of_stream() {
                return TaskOutcome::Requeued(vec![self]);
            }

            if body_ctx.buffer_size() > parser.bytes_consumed() {
                let bytes_to_read = body_ctx.buffer_size() - parser.bytes_consumed();
                let chunk = match source {
                    BodySource::Request => {
                        ctx.get_http_request_body(parser.bytes_consumed(), bytes_to_read)
                    }
                    BodySource::Response => {
                        ctx.get_http_response_body(parser.bytes_consumed(), bytes_to_read)
                    }
                };

                let chunk_bytes = match chunk {
                    Ok(AttributeState::Available(Some(data))) => data,
                    Ok(AttributeState::Available(None)) => {
                        error!("Expected body bytes but got None");
                        return TaskOutcome::Failed;
                    }
                    Ok(AttributeState::Pending) => {
                        return TaskOutcome::Requeued(vec![self]);
                    }
                    Err(e) => {
                        error!("Failed to read body bytes: {e}");
                        return TaskOutcome::Failed;
                    }
                };

                if let Err(e) = parser.feed(&chunk_bytes) {
                    error!("Failed to parse body for '{}': {e}", self.path);
                    return TaskOutcome::Failed;
                }
            }

            if body_ctx.is_end_of_stream() {
                parser.finalize();
                if !parser.is_complete() {
                    let remaining: Vec<&String> = parser.remaining_fields();
                    error!(
                        "Body stream ended without finding fields {:?} for '{}'",
                        remaining, self.path
                    );
                    return TaskOutcome::Failed;
                }
            }

            let body_ctx_mut = match source {
                BodySource::Request => &mut ctx.request_body,
                BodySource::Response => &mut ctx.response_body,
            };
            parser.populate(body_ctx_mut);
        }

        let mut cel_ctx = ctx.cel.new_ctx(&*self);

        if let Some(ref predicate) = self.predicate {
            match predicate.test(ctx, &mut cel_ctx) {
                Ok(AttributeState::Available(true)) => {}
                Ok(AttributeState::Available(false)) => return TaskOutcome::Done,
                Ok(AttributeState::Pending) => {
                    return TaskOutcome::Requeued(vec![self]);
                }
                Err(e) => {
                    error!("Failed to evaluate predicate: {e:?}");
                    return TaskOutcome::Failed;
                }
            }
        }

        let _span = tracing::debug_span!("store").entered();

        let value = match self.expression.eval(ctx, &mut cel_ctx) {
            Ok(AttributeState::Pending) => {
                return TaskOutcome::Requeued(vec![self]);
            }
            Ok(AttributeState::Available(val)) => val,
            Err(e) => {
                error!(
                    "Failed to evaluate store expression for '{}': {e}",
                    self.path
                );
                return TaskOutcome::Failed;
            }
        };

        if self.export_to_host {
            match MessageConverter::cel_value_to_bytes(&value) {
                Ok(bytes) => {
                    if let Err(e) = ctx.set_attribute(&self.path, &bytes) {
                        error!("Failed to store attribute {}: {:?}", self.path, e);
                        return TaskOutcome::Failed;
                    }
                }
                Err(e) => {
                    error!(
                        "Failed to convert value to bytes for '{}': {}",
                        self.path, e
                    );
                    return TaskOutcome::Failed;
                }
            }
        }
        ctx.values.store(self.path.clone(), value);

        if self.terminal {
            TaskOutcome::Terminate(Box::new(SendReplyTask::default()))
        } else {
            TaskOutcome::Done
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::cel::Predicate;
    use crate::data::Expression;
    use crate::kuadrant::MockWasmHost;
    use std::sync::Arc;

    fn make_store_task(
        ctx: &ReqRespCtx,
        predicate: &str,
        expression: &str,
        path: &str,
    ) -> Box<StoreTask> {
        Box::new(
            StoreTask::new(
                ctx,
                "0".to_string(),
                Predicate::new(predicate).unwrap(),
                Expression::new(expression).unwrap(),
                path.to_string(),
                false,
                false,
            )
            .unwrap(),
        )
    }

    #[test]
    fn body_field_extracted_and_stored() {
        let mock_host =
            MockWasmHost::new().with_request_body(br#"{"model":"gpt-4","stream":true}"#);
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));
        ctx.request_body.set_buffer_size(31, true);

        let task = make_store_task(
            &ctx,
            "true",
            "requestBodyJSON('/model')",
            "request.llm.model",
        );

        assert!(matches!(task.apply(&mut ctx), TaskOutcome::Done));

        assert_eq!(
            ctx.values.get("request.llm.model"),
            Some(&cel::Value::String(Arc::new("gpt-4".to_string())))
        );
    }

    #[test]
    fn requeues_when_body_not_available() {
        let mock_host = MockWasmHost::new();
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));

        let task = make_store_task(
            &ctx,
            "true",
            "requestBodyJSON('/model')",
            "request.llm.model",
        );

        assert!(matches!(
            task.apply(&mut ctx),
            TaskOutcome::Requeued(ref tasks) if tasks.len() == 1
        ));
    }

    #[test]
    fn requeues_when_field_not_yet_found() {
        let mock_host = MockWasmHost::new().with_request_body(br#"{"stream":tr"#);
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));
        ctx.request_body.set_buffer_size(12, false);

        let task = make_store_task(
            &ctx,
            "true",
            "requestBodyJSON('/model')",
            "request.llm.model",
        );

        assert!(matches!(
            task.apply(&mut ctx),
            TaskOutcome::Requeued(ref tasks) if tasks.len() == 1
        ));
    }

    #[test]
    fn fails_when_end_of_stream_without_field() {
        let mock_host = MockWasmHost::new().with_request_body(br#"{"stream":true}"#);
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));
        ctx.request_body.set_buffer_size(15, true);

        let task = make_store_task(
            &ctx,
            "true",
            "requestBodyJSON('/model')",
            "request.llm.model",
        );

        assert!(matches!(task.apply(&mut ctx), TaskOutcome::Failed));
    }

    #[test]
    fn response_body_field_extracted() {
        let mock_host = MockWasmHost::new().with_response_body(br#"{"usage":42}"#);
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));
        ctx.response_body.set_buffer_size(12, true);

        let task = make_store_task(&ctx, "true", "responseBodyJSON('/usage')", "response.usage");

        assert!(matches!(task.apply(&mut ctx), TaskOutcome::Done));

        assert_eq!(ctx.values.get("response.usage"), Some(&cel::Value::Int(42)));
    }

    #[test]
    fn predicate_false_skips_store() {
        let mock_host = MockWasmHost::new().with_request_body(br#"{"model":"gpt-4"}"#);
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));
        ctx.request_body.set_buffer_size(18, true);

        let task = make_store_task(
            &ctx,
            "false",
            "requestBodyJSON('/model')",
            "request.llm.model",
        );

        assert!(matches!(task.apply(&mut ctx), TaskOutcome::Done));
        assert!(ctx.values.get("request.llm.model").is_none());
    }

    #[test]
    fn multi_field_expression() {
        let mock_host =
            MockWasmHost::new().with_request_body(br#"{"model":"gpt-4","stream":"yes"}"#);
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));
        ctx.request_body.set_buffer_size(31, true);

        let task = make_store_task(
            &ctx,
            "true",
            "requestBodyJSON('/model') + ':' + requestBodyJSON('/stream')",
            "request.llm.combined",
        );

        assert!(matches!(task.apply(&mut ctx), TaskOutcome::Done));

        assert_eq!(
            ctx.values.get("request.llm.combined"),
            Some(&cel::Value::String(Arc::new("gpt-4:yes".to_string())))
        );
    }

    #[test]
    fn no_body_parser_when_expression_has_no_body_refs() {
        let mock_host = MockWasmHost::new();
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));

        let task = make_store_task(&ctx, "true", "'static_value'", "some.path");

        assert!(matches!(task.apply(&mut ctx), TaskOutcome::Done));

        assert_eq!(
            ctx.values.get("some.path"),
            Some(&cel::Value::String(Arc::new("static_value".to_string())))
        );
    }

    #[test]
    fn invalid_json_pointer_fails_apply() {
        let mock_host = MockWasmHost::new().with_request_body(br#"{"key":"value"}"#);
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));
        ctx.request_body.set_buffer_size(15, true);

        let task = make_store_task(
            &ctx,
            "true",
            "requestBodyJSON('not-a-valid-pointer')",
            "some.path",
        );

        assert!(matches!(task.apply(&mut ctx), TaskOutcome::Failed));
    }

    #[test]
    fn sse_response_body_field_extracted() {
        let sse_data = b"data: {\"id\":\"chunk1\"}\n\ndata: {\"usage\":{\"total_tokens\":42}}\n\ndata: [DONE]\n\n";
        let headers = vec![("content-type".to_string(), "text/event-stream".to_string())];
        let mock_host = MockWasmHost::new()
            .with_response_body(sse_data)
            .with_map("response.headers".to_string(), headers);
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));
        ctx.response_body.set_buffer_size(sse_data.len(), true);

        let task = make_store_task(
            &ctx,
            "true",
            "responseBodyJSON('/usage/total_tokens')",
            "response.usage.total_tokens",
        );

        assert!(matches!(task.apply(&mut ctx), TaskOutcome::Done));

        assert_eq!(
            ctx.values.get("response.usage.total_tokens"),
            Some(&cel::Value::Int(42))
        );
    }

    #[test]
    fn sse_response_missing_field_fails() {
        let sse_data = b"data: {\"other\":1}\n\ndata: [DONE]\n\n";
        let headers = vec![("content-type".to_string(), "text/event-stream".to_string())];
        let mock_host = MockWasmHost::new()
            .with_response_body(sse_data)
            .with_map("response.headers".to_string(), headers);
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));
        ctx.response_body.set_buffer_size(sse_data.len(), true);

        let task = make_store_task(
            &ctx,
            "true",
            "responseBodyJSON('/usage/total_tokens')",
            "response.usage.total_tokens",
        );

        assert!(matches!(task.apply(&mut ctx), TaskOutcome::Failed));
    }

    #[test]
    fn response_json_when_not_sse() {
        let headers = vec![("content-type".to_string(), "application/json".to_string())];
        let mock_host = MockWasmHost::new()
            .with_response_body(br#"{"usage":42}"#)
            .with_map("response.headers".to_string(), headers);
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));
        ctx.response_body.set_buffer_size(12, true);

        let task = make_store_task(&ctx, "true", "responseBodyJSON('/usage')", "response.usage");

        assert!(matches!(task.apply(&mut ctx), TaskOutcome::Done));

        assert_eq!(ctx.values.get("response.usage"), Some(&cel::Value::Int(42)));
    }

    #[test]
    fn multi_chunk_body_parsing() {
        // {"model":"gpt-4","stream":true}
        // The '/stream' field appears later in the JSON
        let mock_host =
            MockWasmHost::new().with_request_body(br#"{"model":"gpt-4","stream":true}"#);
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));

        let mut task: Box<dyn Task> = make_store_task(
            &ctx,
            "true",
            "requestBodyJSON('/stream')",
            "request.llm.stream",
        );

        // Chunk 1: partial body - hasn't reached 'stream' field yet
        ctx.request_body.set_buffer_size(10, false);
        task = match task.apply(&mut ctx) {
            TaskOutcome::Requeued(mut tasks) => {
                assert_eq!(tasks.len(), 1);
                tasks.remove(0)
            }
            _ => unreachable!("Expected requeue after chunk 1"),
        };

        // Chunk 2: more data - still incomplete
        ctx.request_body.set_buffer_size(20, false);
        task = match task.apply(&mut ctx) {
            TaskOutcome::Requeued(mut tasks) => {
                assert_eq!(tasks.len(), 1);
                tasks.remove(0)
            }
            _ => unreachable!("Expected requeue after chunk 2"),
        };

        // Chunk 3: complete body with all fields
        ctx.request_body.set_buffer_size(31, true);
        assert!(matches!(task.apply(&mut ctx), TaskOutcome::Done));

        assert_eq!(
            ctx.values.get("request.llm.stream"),
            Some(&cel::Value::Bool(true))
        );
    }
}
