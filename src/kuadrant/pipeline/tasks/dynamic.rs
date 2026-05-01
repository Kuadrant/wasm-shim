use std::rc::Rc;

use cel::Value;
use tracing::{debug, error};

use crate::configuration::Phase;
use crate::data::attribute::AttributeState;
use crate::data::cel::{Predicate, PredicateVec};
use crate::data::Expression;
use crate::kuadrant::pipeline::blueprint::{Operation, TypedAction};
use crate::kuadrant::pipeline::tasks::{
    HeaderOperation, ModifyHeadersTask, PendingTask, SendReplyTask, StoreDataTask, Task,
    TaskOutcome,
};
use crate::kuadrant::ReqRespCtx;
use crate::record_error;
use crate::services::{cel_value_to_header_pairs, DynamicService};

pub struct DynamicTask {
    task_id: String,
    service: Rc<DynamicService>,
    name: String,
    message_builder: Expression,
    on_reply: Vec<TypedAction>,
    predicates: Vec<Predicate>,
    dependencies: Vec<String>,
    phase: Phase,
}

impl DynamicTask {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ctx: &ReqRespCtx,
        task_id: String,
        service: Rc<DynamicService>,
        name: String,
        message_builder: Expression,
        on_reply: Vec<TypedAction>,
        predicates: Vec<Predicate>,
        dependencies: Vec<String>,
        phase: Phase,
    ) -> Self {
        let _ = predicates.apply(ctx);
        let _ = message_builder.eval(ctx);
        let _ = on_reply.iter().map(|typed_action| {
            let _ = typed_action.predicate.test(ctx);
            match &typed_action.operation {
                Operation::Deny { deny_with } => {
                    let _ = deny_with.eval(ctx);
                }
                Operation::Headers { headers, .. } => {
                    let _ = headers.eval(ctx);
                }
                Operation::Store { data } => {
                    for (_, expr) in data {
                        let _ = expr.eval(ctx);
                    }
                }
            }
        });

        Self {
            task_id,
            service,
            name,
            message_builder,
            on_reply,
            predicates,
            dependencies,
            phase,
        }
    }
}

impl Task for DynamicTask {
    fn id(&self) -> Option<String> {
        Some(self.task_id.clone())
    }

    fn pauses_filter(&self) -> bool {
        true
    }

    fn phase(&self) -> Phase {
        self.phase
    }

    fn dependencies(&self) -> &[String] {
        &self.dependencies
    }

    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        if ctx.phase() != self.phase {
            return TaskOutcome::Requeued(vec![self]);
        }

        match self.predicates.apply(ctx) {
            Ok(AttributeState::Pending) => return TaskOutcome::Requeued(vec![self]),
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
            let cel_value = match self.message_builder.eval_with_ctx(ctx, &mut cel_ctx) {
                Ok(AttributeState::Pending) => return TaskOutcome::Requeued(vec![self]),
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

        TaskOutcome::Deferred {
            token_id,
            pending: Box::new(PendingTask::new(
                self.task_id,
                Box::new(move |ctx| {
                    process_dynamic_response(ctx, &service, &task_id, token_id, &name, &on_reply)
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
    on_reply: &[TypedAction],
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
            _ => continue,
        }

        match &action.operation {
            Operation::Deny { deny_with } => match deny_with.eval_with_ctx(ctx, &mut cel_ctx) {
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
            Operation::Headers { target, headers } => {
                match headers.eval_with_ctx(ctx, &mut cel_ctx) {
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
                }
            }
            Operation::Store { data } => {
                let mut store_items: Vec<(String, Vec<u8>)> = Vec::new();
                for (path, expr) in data {
                    match expr.eval_with_ctx(ctx, &mut cel_ctx) {
                        Ok(AttributeState::Available(val)) => {
                            let bytes = cel_value_to_bytes(&val);
                            store_items.push((path.clone(), bytes));
                        }
                        Ok(AttributeState::Pending) => {
                            error!("Unexpected pending state in onReply store");
                            return TaskOutcome::Failed;
                        }
                        Err(e) => {
                            error!("Failed to evaluate store expression for '{path}': {e}");
                            return TaskOutcome::Failed;
                        }
                    }
                }
                if !store_items.is_empty() {
                    tasks.push(Box::new(StoreDataTask::new(store_items)));
                }
            }
        }
    }

    if tasks.is_empty() {
        TaskOutcome::Done
    } else {
        TaskOutcome::Requeued(tasks)
    }
}

fn cel_value_to_bytes(val: &Value) -> Vec<u8> {
    match val {
        Value::String(s) => s.to_string().into_bytes(),
        Value::Int(n) => n.to_string().into_bytes(),
        Value::UInt(n) => n.to_string().into_bytes(),
        Value::Float(n) => n.to_string().into_bytes(),
        Value::Bool(b) => b.to_string().into_bytes(),
        Value::Null => Vec::new(),
        _ => format!("{val:?}").into_bytes(),
    }
}
