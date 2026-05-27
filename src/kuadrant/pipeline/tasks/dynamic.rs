use std::rc::Rc;

use cel::Value;
use tracing::{debug, error};

use crate::data::attribute::AttributeState;
use crate::data::cel::{Predicate, PredicateVec};
use crate::data::Expression;
use crate::kuadrant::pipeline::blueprint::{Action, Operation};
use crate::kuadrant::pipeline::tasks::{
    HeaderOperation, ModifyHeadersTask, PendingTask, SendReplyTask, StoreTask, Task, TaskOutcome,
};
use crate::kuadrant::ReqRespCtx;
use crate::record_error;
use crate::services::{cel_value_to_header_pairs, DynamicService};

pub struct DynamicTask {
    task_id: String,
    service: Rc<DynamicService>,
    name: String,
    message_builder: Expression,
    on_reply: Vec<Action>,
    predicates: Vec<Predicate>,
    dependencies: Vec<String>,
    is_guard: bool,
}

impl DynamicTask {
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_attributes(
        ctx: &ReqRespCtx,
        task_id: String,
        service: Rc<DynamicService>,
        name: String,
        message_builder: Expression,
        on_reply: Vec<Action>,
        predicates: Vec<Predicate>,
        dependencies: Vec<String>,
        is_guard: bool,
    ) -> Self {
        // Warm up the cache
        let _ = predicates.apply(ctx);
        if let Ok(env) = service.cel_env() {
            let mut cel_ctx = cel::Context::with_env(env);
            let _ = message_builder.eval(ctx, &mut cel_ctx);

            for action in &on_reply {
                let _ = action.predicate.test_with_ctx(ctx, &mut cel_ctx);
                match &action.operation {
                    Operation::Grpc {
                        message_builder,
                        on_reply: nested_on_reply,
                        ..
                    } => {
                        let _ = message_builder.eval(ctx, &mut cel_ctx);
                        for nested_action in nested_on_reply {
                            let _ = nested_action.predicate.test_with_ctx(ctx, &mut cel_ctx);
                        }
                    }
                    Operation::Deny { deny_with } => {
                        let _ = deny_with.eval(ctx, &mut cel_ctx);
                    }
                    Operation::Headers { headers, .. } => {
                        let _ = headers.eval(ctx, &mut cel_ctx);
                    }
                    Operation::Store { expression, .. } => {
                        let _ = expression.eval(ctx, &mut cel_ctx);
                    }
                    Operation::Fail { .. } => {}
                }
            }
        }

        Self {
            task_id,
            service,
            name,
            message_builder,
            on_reply,
            predicates,
            dependencies,
            is_guard,
        }
    }
}

impl Task for DynamicTask {
    fn id(&self) -> Option<String> {
        Some(self.task_id.clone())
    }

    fn dependencies(&self) -> &[String] {
        &self.dependencies
    }

    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        match self.predicates.apply(ctx) {
            Ok(AttributeState::Pending) => {
                return if ctx.response_body.is_end_of_stream() {
                    TaskOutcome::Failed
                } else {
                    TaskOutcome::Requeued(vec![self])
                };
            }
            Ok(AttributeState::Available(false)) => return TaskOutcome::Done,
            Ok(AttributeState::Available(true)) => {}
            Err(e) => {
                error!("Failed to apply predicates: {e:?}");
                return TaskOutcome::Failed;
            }
        }

        let token_id = {
            let _span =
                tracing::debug_span!("dynamic_request", task_id = self.task_id, name = self.name)
                    .entered();

            let env = match self.service.cel_env() {
                Ok(env) => env,
                Err(e) => {
                    error!("Failed to get CEL environment: {e}");
                    return TaskOutcome::Failed;
                }
            };

            let mut cel_ctx = cel::Context::with_env(env);
            let cel_value = match self.message_builder.eval(ctx, &mut cel_ctx) {
                Ok(AttributeState::Pending) => {
                    return if ctx.response_body.is_end_of_stream() {
                        TaskOutcome::Failed
                    } else {
                        TaskOutcome::Requeued(vec![self])
                    };
                }
                Ok(AttributeState::Available(val)) => val,
                Err(e) => {
                    error!("Failed to evaluate message builder: {e}");
                    return TaskOutcome::Failed;
                }
            };

            match self.service.dispatch_value(ctx, &cel_value) {
                Ok(id) => id,
                Err(e) => {
                    error!("Failed to dispatch dynamic service: {e}");
                    return TaskOutcome::Failed;
                }
            }
        };

        let service = self.service.clone();
        let task_id = self.task_id.clone();
        let name = self.name.clone();
        let on_reply = self.on_reply.clone();
        let is_guard = self.is_guard;

        if is_guard {
            ctx.barrier.raise();
        }

        TaskOutcome::Deferred {
            token_id,
            pending: Box::new(PendingTask::new(
                self.task_id,
                Box::new(move |ctx| {
                    let outcome = process_dynamic_response(
                        ctx, &service, &task_id, token_id, &name, &on_reply,
                    );
                    if is_guard {
                        ctx.barrier.lower();
                    }
                    outcome
                }),
            )),
        }
    }
}

fn process_dynamic_response(
    ctx: &mut ReqRespCtx,
    service: &DynamicService,
    task_id: &str,
    token_id: u32,
    name: &str,
    on_reply: &[Action],
) -> TaskOutcome {
    let span = tracing::debug_span!(
        "dynamic_response",
        task_id = task_id,
        token_id = token_id,
        grpc_status_code = tracing::field::Empty,
        otel.status_code = tracing::field::Empty,
        otel.status_message = tracing::field::Empty
    )
    .entered();

    let (status_code, response_size) = match ctx.get_grpc_response_data() {
        Ok(data) => data,
        Err(e) => {
            record_error!("Failed to get gRPC response: {e:?}");
            return TaskOutcome::Failed;
        }
    };
    span.record("grpc_status_code", status_code);

    if status_code != proxy_wasm::types::Status::Ok as u32 {
        record_error!("gRPC status code is not OK");
        return TaskOutcome::Failed;
    }

    if on_reply.is_empty() {
        debug!("No onReply actions, completing");
        return TaskOutcome::Done;
    }

    let mut cel_ctx = match service.response_cel_context(ctx, response_size, name) {
        Ok(c) => c,
        Err(e) => {
            record_error!("Failed to build response context: {e:?}");
            return TaskOutcome::Failed;
        }
    };

    let mut tasks: Vec<Box<dyn Task>> = Vec::new();

    for action in on_reply {
        match action.predicate.test_with_ctx(ctx, &mut cel_ctx) {
            Ok(AttributeState::Available(true)) => {}
            Ok(AttributeState::Available(false)) => continue,
            Ok(AttributeState::Pending) => {
                //todo(@adam-cattermole): if we requeue here, we lose predicates as headers/store/sendreply are not modelled with predicates
            }
            Err(e) => {
                error!("Failed to apply predicates: {e:?}");
                return TaskOutcome::Failed;
            }
        }

        match &action.operation {
            Operation::Deny { deny_with } => match deny_with.eval(ctx, &mut cel_ctx) {
                Ok(AttributeState::Pending) => {
                    error!("Unexpected pending state in onReply deny");
                    return TaskOutcome::Failed;
                }
                Ok(AttributeState::Available(val @ Value::Struct(_))) => {
                    match SendReplyTask::try_from(val) {
                        Ok(task) => {
                            if action.terminal {
                                return TaskOutcome::Terminate(Box::new(task));
                            }
                            tasks.push(Box::new(task));
                        }
                        Err(e) => {
                            error!("Invalid DenyResponse: {e}");
                            return TaskOutcome::Failed;
                        }
                    }
                }
                Ok(AttributeState::Available(other)) => {
                    error!("denyWith must return DenyResponse, got: {other:?}");
                    return TaskOutcome::Failed;
                }
                Err(e) => {
                    error!("Failed to evaluate denyWith expression: {e}");
                    return TaskOutcome::Failed;
                }
            },
            Operation::Headers { target, headers } => match headers.eval(ctx, &mut cel_ctx) {
                Ok(AttributeState::Available(ref val)) => {
                    let pairs = cel_value_to_header_pairs(val);
                    if !pairs.is_empty() {
                        tasks.push(Box::new(ModifyHeadersTask::new(
                            HeaderOperation::Set(pairs.into()),
                            target.clone(),
                        )));
                    }
                }
                Ok(AttributeState::Pending) => {
                    error!("Unexpected pending state in onReply headers");
                    return TaskOutcome::Failed;
                }
                Err(e) => {
                    error!("Failed to evaluate headers expression: {e}");
                    return TaskOutcome::Failed;
                }
            },
            Operation::Store {
                path,
                expression,
                export_to_host,
            } => match expression.eval(ctx, &mut cel_ctx) {
                Ok(AttributeState::Available(val)) => {
                    tasks.push(Box::new(StoreTask::new(path.clone(), val, *export_to_host)));
                }
                Ok(AttributeState::Pending) => {
                    error!("Unexpected pending state in onReply store for '{path}'");
                    return TaskOutcome::Failed;
                }
                Err(e) => {
                    error!("Failed to evaluate store expression for '{path}': {e}");
                    return TaskOutcome::Failed;
                }
            },
            Operation::Fail { log_message } => {
                error!("Action failure: {log_message}");
                return TaskOutcome::Failed;
            }
            Operation::Grpc {
                service,
                var,
                message_builder,
                on_reply: nested_on_reply,
            } => match service {
                crate::services::ServiceInstance::Dynamic(dynamic_service)
                | crate::services::ServiceInstance::Auth(dynamic_service)
                | crate::services::ServiceInstance::RateLimit(dynamic_service)
                | crate::services::ServiceInstance::RateLimitCheck(dynamic_service)
                | crate::services::ServiceInstance::RateLimitReport(dynamic_service) => {
                    let task = Box::new(DynamicTask::new_with_attributes(
                        ctx,
                        action.id.clone(),
                        Rc::clone(dynamic_service),
                        var.clone(),
                        message_builder.clone(),
                        nested_on_reply.clone(),
                        vec![action.predicate.clone()],
                        action.dependencies.clone(),
                        action.is_guard,
                    ));
                    if action.terminal {
                        return TaskOutcome::Terminate(task);
                    }
                    tasks.push(task);
                }
                _ => {
                    error!("Unsupported service type for nested gRPC operation");
                    return TaskOutcome::Failed;
                }
            },
        }
    }

    if tasks.is_empty() {
        TaskOutcome::Done
    } else {
        TaskOutcome::Requeued(tasks)
    }
}
