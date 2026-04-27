use crate::data::attribute::AttributeState;
use crate::data::cel::{Predicate, PredicateVec};
use crate::kuadrant::pipeline::tasks::{PendingTask, SendReplyTask, Task, TaskOutcome};
use crate::kuadrant::ReqRespCtx;
use crate::record_error;
use crate::services::dynamic::converters::MessageConverter;
use crate::services::{DynamicService, Service};
use cel::{Context, Value};
use prost_reflect::{DynamicMessage, ReflectMessage};
use std::rc::Rc;
use tracing::error;

pub struct GrpcMethodTask {
    task_id: String,
    service: Rc<DynamicService>,
    predicates: Vec<Predicate>,
    intention_source: String,
    message_template: String,
    dependencies: Vec<String>,
}

impl GrpcMethodTask {
    pub fn new(
        task_id: String,
        service: Rc<DynamicService>,
        predicates: Vec<Predicate>,
        intention_source: String,
        message_template: String,
        dependencies: Vec<String>,
    ) -> Self {
        Self {
            task_id,
            service,
            predicates,
            intention_source,
            message_template,
            dependencies,
        }
    }
}

impl Task for GrpcMethodTask {
    fn id(&self) -> Option<String> {
        Some(self.task_id.clone())
    }

    fn dependencies(&self) -> &[String] {
        &self.dependencies
    }

    fn pauses_filter(&self) -> bool {
        true
    }

    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        match self.predicates.apply(ctx) {
            Ok(AttributeState::Pending) => return TaskOutcome::Requeued(vec![self]),
            Ok(AttributeState::Available(false)) => return TaskOutcome::Done,
            Ok(AttributeState::Available(true)) => {}
            Err(e) => {
                error!("GrpcMethodTask predicates failed: {e:?}");
                return TaskOutcome::Failed;
            }
        }

        let token_id = {
            let _span = tracing::debug_span!(
                "grpc_method_request",
                task_id = self.task_id,
            )
            .entered();
            match self.service.dispatch_dynamic(ctx, &self.message_template) {
                Ok(id) => id,
                Err(e) => {
                    error!("GrpcMethodTask dispatch failed: {e}");
                    return TaskOutcome::Failed;
                }
            }
        };

        let service = self.service.clone();
        let task_id = self.task_id.clone();
        let intention_source = self.intention_source.clone();

        TaskOutcome::Deferred {
            token_id,
            pending: Box::new(PendingTask::new(
                self.task_id,
                Box::new(move |ctx| {
                    let span = tracing::debug_span!(
                        "grpc_method_response",
                        task_id = task_id,
                        token_id = token_id,
                        grpc_status_code = tracing::field::Empty,
                        otel.status_code = tracing::field::Empty,
                        otel.status_message = tracing::field::Empty
                    )
                    .entered();
                    match ctx.get_grpc_response_data() {
                        Ok((status_code, response_size)) => {
                            span.record("grpc_status_code", status_code);
                            if status_code != proxy_wasm::types::Status::Ok as u32 {
                                record_error!("gRPC status code is not OK");
                                TaskOutcome::Failed
                            } else {
                                match service.get_response(ctx, response_size) {
                                    Ok(response) => {
                                        evaluate_intention(response, &intention_source, ctx)
                                    }
                                    Err(e) => {
                                        record_error!("Failed to get response: {e:?}");
                                        TaskOutcome::Failed
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            record_error!("Failed to get response data: {e:?}");
                            TaskOutcome::Failed
                        }
                    }
                }),
            )),
        }
    }
}

fn evaluate_intention(
    response: DynamicMessage,
    intention_source: &str,
    _ctx: &mut ReqRespCtx,
) -> TaskOutcome {
    let response_type_name = response.descriptor().name().to_string();
    let cel_response = MessageConverter::dynamic_message_to_map(&response);

    // Build a CEL context with the response value available under a camelCase variable name
    // e.g., AssessResponse -> assessResponse
    let var_name = to_camel_case(&response_type_name);

    let mut cel_ctx = Context::default();
    cel_ctx.add_variable_from_value(&var_name, cel_response);

    let program = match cel::Program::compile(intention_source) {
        Ok(p) => p,
        Err(e) => {
            error!("GrpcMethodTask: failed to compile intention: {e}");
            return TaskOutcome::Failed;
        }
    };

    match program.execute(&cel_ctx) {
        Ok(Value::Bool(true)) => TaskOutcome::Done,
        Ok(Value::Bool(false)) => {
            TaskOutcome::Terminate(Box::new(SendReplyTask::new(403, vec![], None)))
        }
        Ok(other) => {
            error!("GrpcMethodTask: intention did not evaluate to bool, got: {other:?}");
            TaskOutcome::Failed
        }
        Err(e) => {
            error!("GrpcMethodTask: intention evaluation failed: {e}");
            TaskOutcome::Failed
        }
    }
}

fn to_camel_case(pascal_case: &str) -> String {
    let mut chars = pascal_case.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_lowercase().collect::<String>() + chars.as_str(),
    }
}
