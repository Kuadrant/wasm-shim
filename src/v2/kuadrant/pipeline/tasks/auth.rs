use crate::envoy::{check_response, CheckResponse};
use crate::v2::data::attribute::AttributeState;
use crate::v2::data::cel::{Predicate, PredicateVec};
use crate::v2::data::Headers;
use crate::v2::kuadrant::pipeline::tasks::{PendingTask, StoreDataTask, Task, TaskOutcome};
use crate::v2::kuadrant::ReqRespCtx;
use crate::v2::services::{AuthService, Service};
use chrono::{DateTime, FixedOffset};
use prost_types::value::Kind;
use std::rc::Rc;

pub struct AuthTask {
    task_id: String,
    service: Rc<AuthService>,
    scope: String,
    predicates: Vec<Predicate>,
    dependencies: Vec<String>,
    is_blocking: bool,
}

impl AuthTask {
    pub fn new(
        task_id: String,
        service: Rc<AuthService>,
        scope: String,
        predicates: Vec<Predicate>,
        dependencies: Vec<String>,
        is_blocking: bool,
    ) -> Self {
        Self {
            task_id,
            service,
            scope,
            predicates,
            dependencies,
            is_blocking,
        }
    }
}

impl Task for AuthTask {
    fn prepare(&self, ctx: &mut ReqRespCtx) -> TaskOutcome {
        let mut has_pending = false;
        let mut has_error = false;

        macro_rules! touch {
            ($path:expr, $type:ty) => {
                match ctx.get_attribute::<$type>($path) {
                    Ok(AttributeState::Available(_)) => {}
                    Ok(AttributeState::Pending) => has_pending = true,
                    Err(_) => has_error = true,
                }
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

        // eval request_data early
        let _ = ctx.eval_request_data();

        // in this case they should all be available
        if has_pending || has_error {
            TaskOutcome::Failed
        } else {
            TaskOutcome::Done
        }
    }

    fn id(&self) -> Option<String> {
        Some(self.task_id.clone())
    }

    fn dependencies(&self) -> &[String] {
        &self.dependencies
    }

    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        match self.predicates.apply(ctx) {
            Ok(AttributeState::Pending) => return TaskOutcome::Requeued(vec![self]),
            Ok(AttributeState::Available(false)) => return TaskOutcome::Done,
            Ok(AttributeState::Available(true)) => {}
            Err(_e) => {
                return TaskOutcome::Failed;
            }
        }

        let token_id = match self.service.dispatch_auth(ctx, &self.scope) {
            Ok(id) => id,
            Err(_e) => {
                return TaskOutcome::Failed;
            }
        };

        let service = self.service.clone();

        TaskOutcome::Deferred {
            token_id,
            pending: PendingTask {
                task_id: Some(self.task_id),
                is_blocking: self.is_blocking,
                process_response: Box::new(move |ctx, status_code, response_size| {
                    if status_code != proxy_wasm::types::Status::Ok as u32 {
                        // todo(refactor): failure case
                        return TaskOutcome::Failed;
                    }

                    match service.get_response(ctx, response_size) {
                        Ok(parsed) => process_auth_response(parsed),
                        Err(_e) => TaskOutcome::Failed,
                    }
                }),
            },
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
            // todo(refactor): Handle empty response
        }
        Some(check_response::HttpResponse::OkResponse(_ok_response)) => {
            // todo(refactor): Add headers, handle headers_to_remove
        }
        Some(check_response::HttpResponse::DeniedResponse(_denied_response)) => {
            // todo(refactor): Send direct response
        }
    }

    if tasks.is_empty() {
        TaskOutcome::Done
    } else {
        TaskOutcome::Requeued(tasks)
    }
}

fn process_metadata(s: &prost_types::Struct, prefix: String) -> Vec<(String, Vec<u8>)> {
    let mut result = Vec::new();

    for (key, value) in &s.fields {
        let current_path = format!("{}\\.{}", prefix, key);

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
                        log::warn!("Unknown Struct field kind for key {}: {:?}", key, kind);
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
                log::warn!("Struct field {} has no kind", key);
            }
        }
    }

    result
}
