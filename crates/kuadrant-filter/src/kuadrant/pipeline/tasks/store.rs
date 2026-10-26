mod body_parser;
mod json_body_parser;
pub(super) mod sse_body_parser;

use body_parser::BodyParser;
use cel::Value;
use json_body_parser::JsonBodyParser;
use sse_body_parser::SseBodyParser;
use tracing::{debug, error, warn};

use crate::data::attribute::{AttributeError, AttributeState};
use crate::data::cel::{BodyFieldGroup, Predicate};
use crate::data::Expression;
use crate::data::Headers;
use crate::kuadrant::pipeline::tasks::{SendReplyTask, Task, TaskOutcome};
use crate::kuadrant::{PathReservation, ReqRespCtx};
use crate::metrics::METRICS;
use crate::services::MessageConverter;

#[derive(Clone, Copy)]
enum BodySource {
    Request,
    Response,
}

enum BodyParseState {
    NotNeeded,
    Pending(BodySource, Vec<BodyFieldGroup>),
    Active(BodySource, Box<dyn BodyParser>),
}

fn dedup_groups(groups: &mut Vec<BodyFieldGroup>) {
    groups.sort_by(|a, b| a.key.cmp(&b.key));
    groups.dedup_by(|a, b| a.key == b.key);
}

pub struct StoreTask {
    task_id: String,
    predicate: Option<Predicate>,
    expression: Expression,
    path: String,
    export_to_host: bool,
    export_format: Option<crate::configuration::ExportFormat>,
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
        export_format: Option<crate::configuration::ExportFormat>,
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
            export_format,
            terminal,
            body_parse_state,
            _reservation: reservation,
        })
    }

    fn initial_body_state(predicate: &Predicate, expression: &Expression) -> BodyParseState {
        let mut request_groups: Vec<BodyFieldGroup> = Vec::new();
        let mut response_groups: Vec<BodyFieldGroup> = Vec::new();

        request_groups.extend_from_slice(predicate.request_body_groups());
        request_groups.extend_from_slice(expression.request_body_groups());

        response_groups.extend_from_slice(predicate.response_body_groups());
        response_groups.extend_from_slice(expression.response_body_groups());

        if !request_groups.is_empty() {
            dedup_groups(&mut request_groups);
            BodyParseState::Pending(BodySource::Request, request_groups)
        } else if !response_groups.is_empty() {
            dedup_groups(&mut response_groups);
            BodyParseState::Pending(BodySource::Response, response_groups)
        } else {
            BodyParseState::NotNeeded
        }
    }

    fn create_parser(
        ctx: &ReqRespCtx,
        source: BodySource,
        groups: &[BodyFieldGroup],
    ) -> Result<Option<Box<dyn BodyParser>>, AttributeError> {
        match source {
            BodySource::Request => {
                let parser = JsonBodyParser::new(groups.to_vec()).map_err(|e| {
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
                    Ok(Some(Box::new(SseBodyParser::new(groups.to_vec()))))
                } else {
                    let parser = JsonBodyParser::new(groups.to_vec()).map_err(|e| {
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

            if body_ctx.is_end_of_stream() && !parser.remaining_fields().is_empty() {
                if let Err(e) = parser.finalize() {
                    error!("Failed to finalize body parser for '{}': {e}", self.path);
                    return TaskOutcome::Failed;
                }
                let remaining = parser.remaining_fields();
                if !remaining.is_empty() {
                    let candidates: Vec<&[String]> =
                        remaining.iter().map(|g| g.candidates.as_slice()).collect();
                    debug!(
                        "No candidate resolved for field(s) {:?} in '{}': body stream ended \
                        without a match; the request proceeds and this value is skipped",
                        candidates, self.path
                    );
                    METRICS.body_extraction_misses().increment();
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

        let protobuf_value_export = self.export_format
            == Some(crate::configuration::ExportFormat::ProtobufValue);
        let depends_on_sensitive_value = self.expression.has_request_headers_dependency()
            || self
                .expression
                .references_any_path(ctx.values.sensitive_paths());

        if self.export_to_host && protobuf_value_export && depends_on_sensitive_value {
            warn!(
                "Skipping typed host export for '{}' because its value depends on sensitive request data",
                self.path
            );
        } else if self.export_to_host && protobuf_value_export && matches!(&value, Value::Null) {
            // Missing optional values are represented by an absent filter-state path.
            if let Err(e) = ctx.clear_attribute(&self.path) {
                error!("Failed to clear host attribute {}: {:?}", self.path, e);
                return TaskOutcome::Failed;
            }
        } else if self.export_to_host {
            let exported = if protobuf_value_export {
                MessageConverter::cel_value_to_protobuf_value_bytes(&value)
            } else {
                MessageConverter::cel_value_to_bytes(&value)
            };
            match exported {
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
        if depends_on_sensitive_value {
            ctx.values.mark_sensitive(self.path.clone());
        }

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
    use crate::data::attribute::{AttributeState, Path};
    use crate::data::cel::Predicate;
    use crate::data::Expression;
    use crate::kuadrant::MockWasmHost;
    use prost::Message;
    use std::sync::Arc;

    fn make_store_task(
        ctx: &ReqRespCtx,
        predicate: &str,
        expression: &str,
        path: &str,
    ) -> Box<StoreTask> {
        make_store_task_with_export(ctx, predicate, expression, path, false)
    }

    fn make_store_task_with_export(
        ctx: &ReqRespCtx,
        predicate: &str,
        expression: &str,
        path: &str,
        export_to_host: bool,
    ) -> Box<StoreTask> {
        make_store_task_with_export_format(ctx, predicate, expression, path, export_to_host, None)
    }

    fn make_store_task_with_export_format(
        ctx: &ReqRespCtx,
        predicate: &str,
        expression: &str,
        path: &str,
        export_to_host: bool,
        export_format: Option<crate::configuration::ExportFormat>,
    ) -> Box<StoreTask> {
        Box::new(
            StoreTask::new(
                ctx,
                "0".to_string(),
                Predicate::new(predicate).unwrap(),
                Expression::new(expression).unwrap(),
                path.to_string(),
                export_to_host,
                export_format,
                false,
            )
            .unwrap(),
        )
    }

    #[test]
    fn exported_ocsf_map_and_list_are_typed_protobuf_values() {
        let mock_host = Arc::new(MockWasmHost::new());
        let mut ctx = ReqRespCtx::new(mock_host.clone());
        let task = make_store_task_with_export_format(
            &ctx,
            "true",
            "{'name': 'alice', 'groups': [{'name': 'admin'}]}",
            "ocsf.actor.user.groups",
            true,
            Some(crate::configuration::ExportFormat::ProtobufValue),
        );

        assert!(matches!(task.apply(&mut ctx), TaskOutcome::Done));

        let property_path = Path::new(vec!["filter_state", "wasm.kuadrant.ocsf.actor.user.groups"]);
        let bytes = mock_host
            .get_property(&property_path)
            .expect("OCSF export is written to host state");
        let exported = prost_types::Value::decode(bytes.as_slice())
            .expect("OCSF map is encoded as google.protobuf.Value");
        assert!(matches!(
            &exported.kind,
            Some(prost_types::value::Kind::StructValue(_))
        ));
        let Some(prost_types::value::Kind::StructValue(record)) = exported.kind else {
            return;
        };
        let groups_kind = record
            .fields
            .get("groups")
            .and_then(|value| value.kind.as_ref());
        assert!(matches!(
            groups_kind,
            Some(prost_types::value::Kind::ListValue(_))
        ));
        let Some(prost_types::value::Kind::ListValue(groups)) = groups_kind else {
            return;
        };
        let group_kind = groups.values.first().and_then(|value| value.kind.as_ref());
        assert!(matches!(
            group_kind,
            Some(prost_types::value::Kind::StructValue(_))
        ));
        let Some(prost_types::value::Kind::StructValue(group)) = group_kind else {
            return;
        };
        assert!(matches!(
            group.fields.get("name").and_then(|value| value.kind.as_ref()),
            Some(prost_types::value::Kind::StringValue(name)) if name == "admin"
        ));
    }

    #[test]
    fn missing_ocsf_value_is_not_exported() {
        let mock_host = Arc::new(MockWasmHost::new());
        let mut ctx = ReqRespCtx::new(mock_host.clone());
        let task = make_store_task_with_export_format(
            &ctx,
            "false",
            "'optional-value'",
            "ocsf.actor.user.name",
            true,
            Some(crate::configuration::ExportFormat::ProtobufValue),
        );

        assert!(matches!(task.apply(&mut ctx), TaskOutcome::Done));
        assert!(mock_host
            .get_property(&Path::new(vec![
                "filter_state",
                "wasm.kuadrant.ocsf.actor.user.name",
            ]))
            .is_none());

        let null_task =
            make_store_task_with_export_format(
                &ctx,
                "true",
                "null",
                "ocsf.target.resource.name",
                true,
                Some(crate::configuration::ExportFormat::ProtobufValue),
            );
        assert!(matches!(null_task.apply(&mut ctx), TaskOutcome::Done));
        assert!(mock_host
            .get_property(&Path::new(vec![
                "filter_state",
                "wasm.kuadrant.ocsf.target.resource.name",
            ]))
            .is_none());
    }

    #[test]
    fn null_ocsf_value_clears_a_previous_export() {
        let mock_host = Arc::new(MockWasmHost::new());
        let mut ctx = ReqRespCtx::new(mock_host.clone());
        let path = "ocsf.target.resource.name";
        let property_path = Path::new(vec![
            "filter_state",
            "wasm.kuadrant.ocsf.target.resource.name",
        ]);

        let initial = make_store_task_with_export_format(
            &ctx,
            "true",
            "'model'",
            path,
            true,
            Some(crate::configuration::ExportFormat::ProtobufValue),
        );
        assert!(matches!(initial.apply(&mut ctx), TaskOutcome::Done));
        assert!(mock_host.get_property(&property_path).is_some());

        let missing = make_store_task_with_export_format(
            &ctx,
            "true",
            "null",
            path,
            true,
            Some(crate::configuration::ExportFormat::ProtobufValue),
        );
        assert!(matches!(missing.apply(&mut ctx), TaskOutcome::Done));
        assert!(mock_host.get_property(&property_path).is_none());
        assert!(matches!(
            ctx.get_attribute::<String>(path),
            Ok(AttributeState::Available(None))
        ));
    }

    #[test]
    fn exported_ocsf_scalars_are_typed_protobuf_values() {
        use prost_types::value::Kind;

        let cases = ["'alice'", "42", "42.5", "true"];
        for expression in cases {
            let mock_host = Arc::new(MockWasmHost::new());
            let mut ctx = ReqRespCtx::new(mock_host.clone());
            let task = make_store_task_with_export_format(
                &ctx,
                "true",
                expression,
                "ocsf.actor.user.name",
                true,
                Some(crate::configuration::ExportFormat::ProtobufValue),
            );

            assert!(matches!(task.apply(&mut ctx), TaskOutcome::Done));

            let bytes = mock_host
                .get_property(&Path::new(vec![
                    "filter_state",
                    "wasm.kuadrant.ocsf.actor.user.name",
                ]))
                .expect("OCSF scalar export is written to host state");
            let exported = prost_types::Value::decode(bytes.as_slice())
                .expect("OCSF scalar is encoded as google.protobuf.Value");
            let matches_expected_type = match expression {
                "'alice'" => {
                    matches!(exported.kind, Some(Kind::StringValue(name)) if name == "alice")
                }
                "42" => matches!(exported.kind, Some(Kind::NumberValue(value)) if value == 42.0),
                "42.5" => matches!(exported.kind, Some(Kind::NumberValue(value)) if value == 42.5),
                "true" => matches!(exported.kind, Some(Kind::BoolValue(true))),
                _ => false,
            };
            assert!(
                matches_expected_type,
                "CEL value retains its protobuf Value type"
            );
        }
    }

    #[test]
    fn authorization_header_values_are_not_exported_to_ocsf() {
        let authorization = "Bearer sensitive-authorization-value";
        let mock_host = Arc::new(MockWasmHost::new().with_map(
            "request.headers".to_string(),
            vec![("authorization".to_string(), authorization.to_string())],
        ));
        let mut ctx = ReqRespCtx::new(mock_host.clone());
        let direct = make_store_task_with_export_format(
            &ctx,
            "true",
            "request.headers.authorization",
            "ocsf.actor.user.name",
            true,
            Some(crate::configuration::ExportFormat::ProtobufValue),
        );

        assert!(matches!(direct.apply(&mut ctx), TaskOutcome::Done));
        assert!(mock_host
            .get_property(&Path::new(vec![
                "filter_state",
                "wasm.kuadrant.ocsf.actor.user.name",
            ]))
            .is_none());

        let derived = make_store_task_with_export_format(
            &ctx,
            "true",
            "ocsf.actor.user.name",
            "ocsf.actor.user.email",
            true,
            Some(crate::configuration::ExportFormat::ProtobufValue),
        );
        assert!(matches!(derived.apply(&mut ctx), TaskOutcome::Done));
        assert!(mock_host
            .get_property(&Path::new(vec![
                "filter_state",
                "wasm.kuadrant.ocsf.actor.user.email",
            ]))
            .is_none());

        let stored_secret = make_store_task_with_export(
            &ctx,
            "true",
            "request.headers.authorization",
            "authorizationToken",
            false,
        );
        assert!(matches!(stored_secret.apply(&mut ctx), TaskOutcome::Done));

        let aliased = make_store_task_with_export_format(
            &ctx,
            "true",
            "authorizationToken",
            "ocsf.actor.user.uid",
            true,
            Some(crate::configuration::ExportFormat::ProtobufValue),
        );
        assert!(matches!(aliased.apply(&mut ctx), TaskOutcome::Done));
        assert!(mock_host
            .get_property(&Path::new(vec![
                "filter_state",
                "wasm.kuadrant.ocsf.actor.user.uid",
            ]))
            .is_none());
    }

    #[test]
    fn non_ocsf_export_keeps_legacy_struct_encoding() {
        let mock_host = Arc::new(MockWasmHost::new());
        let mut ctx = ReqRespCtx::new(mock_host.clone());
        let task = make_store_task_with_export(
            &ctx,
            "true",
            "{'name': 'alice'}",
            "auth.identity.user",
            true,
        );

        assert!(matches!(task.apply(&mut ctx), TaskOutcome::Done));

        let property_path = Path::new(vec!["filter_state", "wasm.kuadrant.auth.identity.user"]);
        let bytes = mock_host
            .get_property(&property_path)
            .expect("non-OCSF export is written to host state");
        let exported = prost_types::Struct::decode(bytes.as_slice())
            .expect("non-OCSF map retains the legacy google.protobuf.Struct encoding");
        assert!(matches!(
            exported.fields.get("name").and_then(|value| value.kind.as_ref()),
            Some(prost_types::value::Kind::StringValue(name)) if name == "alice"
        ));
    }

    #[test]
    fn protobuf_value_export_works_for_non_ocsf_paths() {
        let mock_host = Arc::new(MockWasmHost::new());
        let mut ctx = ReqRespCtx::new(mock_host.clone());
        let task = make_store_task_with_export_format(
            &ctx,
            "true",
            "{'groups': [{'name': 'admin'}], 'enabled': true}",
            "custom.audit.metadata",
            true,
            Some(crate::configuration::ExportFormat::ProtobufValue),
        );

        assert!(matches!(task.apply(&mut ctx), TaskOutcome::Done));

        let property_path = Path::new(vec!["filter_state", "wasm.kuadrant.custom.audit.metadata"]);
        let bytes = mock_host
            .get_property(&property_path)
            .expect("typed generic export is written to host state");
        let exported = prost_types::Value::decode(bytes.as_slice())
            .expect("typed generic export uses google.protobuf.Value");
        assert!(matches!(
            &exported.kind,
            Some(prost_types::value::Kind::StructValue(_))
        ));
        let Some(prost_types::value::Kind::StructValue(record)) = exported.kind else {
            return;
        };
        assert!(matches!(
            record.fields.get("enabled").and_then(|value| value.kind.as_ref()),
            Some(prost_types::value::Kind::BoolValue(true))
        ));
        assert!(matches!(
            record
                .fields
                .get("groups")
                .and_then(|value| value.kind.as_ref()),
            Some(prost_types::value::Kind::ListValue(_))
        ));
    }

    #[test]
    fn typed_export_does_not_write_when_predicate_is_false() {
        let mock_host = Arc::new(MockWasmHost::new());
        let mut ctx = ReqRespCtx::new(mock_host.clone());
        let task = make_store_task_with_export_format(
            &ctx,
            "false",
            "'should-not-export'",
            "custom.audit.identity",
            true,
            Some(crate::configuration::ExportFormat::ProtobufValue),
        );

        assert!(matches!(task.apply(&mut ctx), TaskOutcome::Done));
        assert!(mock_host
            .get_property(&Path::new(vec!["filter_state", "wasm.kuadrant.custom.audit.identity"]))
            .is_none());
    }

    #[test]
    fn typed_export_does_not_export_authorization_for_generic_paths() {
        let authorization = "Bearer sensitive-authorization-value";
        let mock_host = Arc::new(MockWasmHost::new().with_map(
            "request.headers".to_string(),
            vec![("authorization".to_string(), authorization.to_string())],
        ));
        let mut ctx = ReqRespCtx::new(mock_host.clone());
        let task = make_store_task_with_export_format(
            &ctx,
            "true",
            "request.headers.authorization",
            "custom.audit.identity",
            true,
            Some(crate::configuration::ExportFormat::ProtobufValue),
        );

        assert!(matches!(task.apply(&mut ctx), TaskOutcome::Done));
        assert!(mock_host
            .get_property(&Path::new(vec!["filter_state", "wasm.kuadrant.custom.audit.identity"]))
            .is_none());
    }

    #[test]
    fn multi_field_via_separate_calls() {
        let response_body = br#"{"usage":{"total_tokens":18,"input_tokens":10,"output_tokens":8}}"#;
        let mock_host = MockWasmHost::new().with_response_body(response_body);
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));
        ctx.response_body.set_buffer_size(response_body.len(), true);

        let task = make_store_task(
            &ctx,
            "true",
            r#"{
                "total_tokens": responseBodyJSON(["/usage/total_tokens"], "number"),
                "input_tokens": responseBodyJSON(["/usage/input_tokens"], "number"),
                "output_tokens": responseBodyJSON(["/usage/output_tokens"], "number")
            }"#,
            "kuadrant.internal.response.body",
        );

        assert!(matches!(task.apply(&mut ctx), TaskOutcome::Done));

        let stored = ctx
            .values
            .get("kuadrant.internal.response.body")
            .expect("value should be stored");
        match stored {
            cel::Value::Map(m) => {
                let key = |s: &str| cel::objects::Key::String(Arc::new(s.to_string()));
                assert_eq!(m.get(&key("total_tokens")), Some(&cel::Value::Int(18)));
                assert_eq!(m.get(&key("input_tokens")), Some(&cel::Value::Int(10)));
                assert_eq!(m.get(&key("output_tokens")), Some(&cel::Value::Int(8)));
            }
            other => unreachable!("expected a map, got {other:?}"),
        }
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
    fn ordered_candidate_list_falls_back_to_second_pointer() {
        // Simulates the RFC 0024 use case: a Gemini-shaped body has no
        // `/usage/total_tokens`, but the list's second candidate matches.
        let mock_host =
            MockWasmHost::new().with_response_body(br#"{"usageMetadata":{"totalTokenCount":7}}"#);
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));
        ctx.response_body.set_buffer_size(40, true);

        let task = make_store_task(
            &ctx,
            "true",
            "responseBodyJSON(['/usage/total_tokens', '/usageMetadata/totalTokenCount'], 'number')",
            "response.usage.total_tokens",
        );

        assert!(matches!(task.apply(&mut ctx), TaskOutcome::Done));
        assert_eq!(
            ctx.values.get("response.usage.total_tokens"),
            Some(&cel::Value::Int(7))
        );
    }

    #[test]
    fn ordered_candidate_list_resolves_before_end_of_stream() {
        // The second-listed candidate happens to appear (and fully resolve)
        // before the first-listed one arrives at all. Resolution is
        // first-to-arrive, not list order, so the task completes as soon as
        // it's seen -- without waiting for the rest of the body, or even for
        // end-of-stream -- taking advantage of the streaming parse.
        let full_body = br#"{"usageMetadata":{"totalTokenCount":7},"usage":{"total_tokens":999}}"#;
        let prefix_len = full_body.len() - br#","usage":{"total_tokens":999}}"#.len();

        let mock_host = MockWasmHost::new().with_response_body(full_body);
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));
        ctx.response_body.set_buffer_size(prefix_len, false);

        let task = make_store_task(
            &ctx,
            "true",
            "responseBodyJSON(['/usage/total_tokens', '/usageMetadata/totalTokenCount'], 'number')",
            "response.usage.total_tokens",
        );

        assert!(matches!(task.apply(&mut ctx), TaskOutcome::Done));
        assert_eq!(
            ctx.values.get("response.usage.total_tokens"),
            Some(&cel::Value::Int(7))
        );
    }

    #[test]
    fn ordered_candidate_list_skips_non_numeric_candidate() {
        // The first candidate is present but not a number; the type hint makes it
        // fall through to the second, numeric candidate instead of stopping there.
        let mock_host = MockWasmHost::new()
            .with_response_body(br#"{"model":"gpt-4","usage":{"total_tokens":42}}"#);
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));
        ctx.response_body.set_buffer_size(46, true);

        let task = make_store_task(
            &ctx,
            "true",
            "responseBodyJSON(['/model', '/usage/total_tokens'], 'number')",
            "response.usage.total_tokens",
        );

        assert!(matches!(task.apply(&mut ctx), TaskOutcome::Done));
        assert_eq!(
            ctx.values.get("response.usage.total_tokens"),
            Some(&cel::Value::Int(42))
        );
    }

    #[test]
    fn ordered_candidate_list_fails_at_eos_when_no_candidate_resolves() {
        let mock_host = MockWasmHost::new().with_response_body(br#"{"other":1}"#);
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));
        ctx.response_body.set_buffer_size(11, true);

        let task = make_store_task(
            &ctx,
            "true",
            "responseBodyJSON(['/usage/total_tokens', '/usageMetadata/totalTokenCount'], 'number')",
            "response.usage.total_tokens",
        );

        assert!(matches!(task.apply(&mut ctx), TaskOutcome::Failed));
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
