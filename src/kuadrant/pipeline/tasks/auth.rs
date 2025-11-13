use crate::data::attribute::AttributeState;
use crate::data::cel::{Predicate, PredicateVec};
use crate::data::Headers;
use crate::envoy::{check_response, CheckResponse, HeaderValueOption};
use crate::kuadrant::pipeline::tasks::{
    HeaderOperation, HeadersType, ModifyHeadersTask, PendingTask, SendReplyTask, StoreDataTask,
    Task, TaskOutcome,
};
use crate::kuadrant::ReqRespCtx;
use crate::services::{AuthService, Service};
use chrono::{DateTime, FixedOffset};
use log::{error, warn};
use prost_types::value::Kind;
use std::rc::Rc;

pub struct AuthTask {
    task_id: String,
    service: Rc<AuthService>,
    scope: String,
    predicates: Vec<Predicate>,
    dependencies: Vec<String>,
    pauses_filter: bool,
}

impl AuthTask {
    fn new(
        task_id: String,
        service: Rc<AuthService>,
        scope: String,
        predicates: Vec<Predicate>,
        dependencies: Vec<String>,
        pauses_filter: bool,
    ) -> Self {
        Self {
            task_id,
            service,
            scope,
            predicates,
            dependencies,
            pauses_filter,
        }
    }

    pub fn new_with_attributes(
        ctx: &ReqRespCtx,
        task_id: String,
        service: Rc<AuthService>,
        scope: String,
        predicates: Vec<Predicate>,
        dependencies: Vec<String>,
        pauses_filter: bool,
    ) -> Self {
        macro_rules! touch {
            ($path:expr, $type:ty) => {
                let _ = ctx.get_attribute::<$type>($path);
            };
        }

        // these are required to build a CheckRequest
        touch!("request.headers", Headers);
        touch!("request.host", String);
        touch!("request.method", String);
        touch!("request.scheme", String);
        touch!("request.path", String);
        touch!("request.protocol", String);
        touch!("request.time", DateTime<FixedOffset>);
        touch!("destination.address", String);
        touch!("destination.port", i64);
        touch!("source.address", String);
        touch!("source.port", i64);

        let _ = predicates.apply(ctx);

        // eval request_data early
        let _ = ctx.eval_request_data();

        Self::new(
            task_id,
            service,
            scope,
            predicates,
            dependencies,
            pauses_filter,
        )
    }
}

impl Task for AuthTask {
    fn id(&self) -> Option<String> {
        Some(self.task_id.clone())
    }

    fn pauses_filter(&self, _ctx: &ReqRespCtx) -> bool {
        self.pauses_filter
    }

    fn dependencies(&self) -> &[String] {
        &self.dependencies
    }

    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        match self.predicates.apply(ctx) {
            Ok(AttributeState::Pending) => return TaskOutcome::Requeued(vec![self]),
            Ok(AttributeState::Available(false)) => return TaskOutcome::Done,
            Ok(AttributeState::Available(true)) => {}
            Err(e) => {
                error!("Failed to apply predicates: {e:?}");
                return TaskOutcome::Failed;
            }
        }

        let token_id = match self.service.dispatch_auth(ctx, &self.scope) {
            Ok(id) => id,
            Err(e) => {
                error!("Failed to dispatch auth: {e:?}");
                return TaskOutcome::Failed;
            }
        };

        let service = self.service.clone();

        TaskOutcome::Deferred {
            token_id,
            pending: Box::new(PendingTask {
                task_id: self.task_id,
                pauses_filter: self.pauses_filter,
                process_response: Box::new(move |ctx| match ctx.get_grpc_response_data() {
                    Ok((status_code, response_size)) => {
                        if status_code != proxy_wasm::types::Status::Ok as u32 {
                            TaskOutcome::Failed
                        } else {
                            match service.get_response(ctx, response_size) {
                                Ok(parsed) => process_auth_response(parsed),
                                Err(e) => {
                                    error!("Failed to get response: {e:?}");
                                    TaskOutcome::Failed
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error!("Failed to get response: {e:?}");
                        TaskOutcome::Failed
                    }
                }),
            }),
        }
    }
}

fn process_auth_response(response: CheckResponse) -> TaskOutcome {
    let mut tasks: Vec<Box<dyn Task>> = Vec::new();

    // Create store task if dynamic metadata present
    if let Some(ref dynamic_metadata) = response.dynamic_metadata {
        let data = process_metadata(dynamic_metadata, "auth".to_string());
        if !data.is_empty() {
            tasks.push(Box::new(StoreDataTask::new(data)));
        }
    }

    match response.http_response {
        None => {
            warn!("Auth response contained no http_response");
            return TaskOutcome::Failed;
        }
        Some(check_response::HttpResponse::OkResponse(ok_response)) => {
            // Check for unsupported fields
            if !ok_response.response_headers_to_add.is_empty() {
                warn!("Unsupported field 'response_headers_to_add' in OkHttpResponse");
                return TaskOutcome::Failed;
            }
            if !ok_response.headers_to_remove.is_empty() {
                warn!("Unsupported field 'headers_to_remove' in OkHttpResponse");
                return TaskOutcome::Failed;
            }
            if !ok_response.query_parameters_to_set.is_empty() {
                warn!("Unsupported field 'query_parameters_to_set' in OkHttpResponse");
                return TaskOutcome::Failed;
            }
            if !ok_response.query_parameters_to_remove.is_empty() {
                warn!("Unsupported field 'query_parameters_to_remove' in OkHttpResponse");
                return TaskOutcome::Failed;
            }

            // Add request headers if present
            if !ok_response.headers.is_empty() {
                let headers = from_envoy_headers(&ok_response.headers);
                tasks.push(Box::new(ModifyHeadersTask::new(
                    HeaderOperation::Append(headers),
                    HeadersType::HttpRequestHeaders,
                )));
            }
        }
        Some(check_response::HttpResponse::DeniedResponse(denied_response)) => {
            let status_code = denied_response
                .status
                .as_ref()
                .map(|s| s.code as u32)
                .unwrap_or(403);

            let headers = from_envoy_headers(&denied_response.headers);

            let body = if denied_response.body.is_empty() {
                None
            } else {
                Some(denied_response.body.clone())
            };

            return TaskOutcome::Terminate(Box::new(SendReplyTask::new(
                status_code,
                headers.into_inner(),
                body,
            )));
        }
    }

    if tasks.is_empty() {
        TaskOutcome::Done
    } else {
        TaskOutcome::Requeued(tasks)
    }
}

fn from_envoy_headers(headers: &[HeaderValueOption]) -> Headers {
    let vec: Vec<(String, String)> = headers
        .iter()
        .filter_map(|header| {
            header
                .header
                .as_ref()
                .map(|hv| (hv.key.to_owned(), hv.value.to_owned()))
        })
        .collect();
    vec.into()
}

fn process_metadata(s: &prost_types::Struct, prefix: String) -> Vec<(String, Vec<u8>)> {
    let mut result = Vec::new();

    for (key, value) in &s.fields {
        let current_path = format!("{}.{}", prefix, key);

        match &value.kind {
            Some(Kind::StructValue(nested_struct)) => {
                result.extend(process_metadata(nested_struct, current_path));
            }
            Some(kind) => {
                let json = match kind {
                    Kind::StringValue(s) => Some(serde_json::Value::String(s.clone())),
                    Kind::BoolValue(b) => Some(serde_json::Value::Bool(*b)),
                    Kind::NullValue(_) => Some(serde_json::Value::Null),
                    Kind::NumberValue(n) => Some(serde_json::json!(n)),
                    Kind::StructValue(_) => unreachable!(),
                    _ => {
                        warn!("Unknown Struct field kind for key {}: {:?}", key, kind);
                        None
                    }
                };

                if let Some(v) = json {
                    if let Ok(serialized) = serde_json::to_string(&v) {
                        result.push((current_path, serialized.into_bytes()));
                    }
                }
            }
            None => {
                warn!("Struct field {} has no kind", key);
            }
        }
    }

    result
}
