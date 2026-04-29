use std::rc::Rc;

use cel::{Program, Value};
use tracing::{debug, error};

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
}

impl DynamicTask {
    pub fn new(
        task_id: String,
        service: Rc<DynamicService>,
        name: String,
        message_builder: Expression,
        on_reply: Vec<TypedAction>,
        predicates: Vec<Predicate>,
        dependencies: Vec<String>,
    ) -> Self {
        Self {
            task_id,
            service,
            name,
            message_builder,
            on_reply,
            predicates,
            dependencies,
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

    let cel_ctx = match service.response_cel_context(ctx, response_size, name) {
        Ok(c) => c,
        Err(e) => {
            record_error!("Failed to build response context: {e:?}");
            return TaskOutcome::Failed;
        }
    };

    let mut tasks: Vec<Box<dyn Task>> = Vec::new();

    for action in on_reply {
        match action.predicate.test(ctx) {
            Ok(AttributeState::Available(true)) => {}
            Ok(AttributeState::Available(false)) => continue,
            _ => continue,
        }

        match &action.operation {
            Operation::Deny { deny_with } => match compile_and_eval(deny_with, &cel_ctx) {
                Ok(val @ Value::Struct(_)) => match SendReplyTask::try_from(val) {
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
                },
                Ok(other) => {
                    error!("denyWith must return DenyResponse, got: {other:?}");
                    return TaskOutcome::Failed;
                }
                Err(e) => {
                    error!("Failed to evaluate denyWith expression: {e}");
                    return TaskOutcome::Failed;
                }
            },
            Operation::Headers { target, headers } => match compile_and_eval(headers, &cel_ctx) {
                Ok(ref val) => {
                    let pairs = cel_value_to_header_pairs(val);
                    if !pairs.is_empty() {
                        tasks.push(Box::new(ModifyHeadersTask::new(
                            HeaderOperation::Set(pairs.into()),
                            target.clone(),
                        )));
                    }
                }
                Err(e) => {
                    error!("Failed to evaluate headers expression: {e}");
                    return TaskOutcome::Failed;
                }
            },
            Operation::Store { data } => {
                let mut store_items: Vec<(String, Vec<u8>)> = Vec::new();
                for (path, expr) in data {
                    match compile_and_eval(expr, &cel_ctx) {
                        Ok(val) => {
                            let bytes = cel_value_to_bytes(&val);
                            store_items.push((path.clone(), bytes));
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

fn compile_and_eval(expression: &str, ctx: &cel::Context) -> Result<Value, String> {
    let program = Program::compile(expression).map_err(|e| format!("CEL compile error: {}", e))?;
    program
        .execute(ctx)
        .map_err(|e| format!("CEL execution error: {}", e))
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
