use std::rc::Rc;

use tracing::{debug, error};

use crate::data::attribute::AttributeState;
use crate::data::cel::Predicate;
use crate::data::Expression;
use crate::kuadrant::pipeline::tasks::{PendingTask, Task, TaskOutcome};
use crate::kuadrant::ReqRespCtx;
use crate::record_error;
use crate::services::{DescriptorConverter, DynamicService};

pub struct DynamicTask {
    task_id: String,
    service: Rc<DynamicService>,
    name: String,
    message_builder: Expression,
    on_reply: Vec<Box<dyn Task>>,
    predicate: Predicate,
    dependencies: Vec<String>,
    is_guard: bool,
}

impl DynamicTask {
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_attributes(
        ctx: &mut ReqRespCtx,
        task_id: String,
        service: Rc<DynamicService>,
        name: String,
        message_builder: Expression,
        on_reply: Vec<Box<dyn Task>>,
        predicate: Predicate,
        dependencies: Vec<String>,
        is_guard: bool,
    ) -> Self {
        let task = Self {
            task_id,
            service,
            name,
            message_builder,
            on_reply,
            predicate,
            dependencies,
            is_guard,
        };

        task.warm(ctx);
        task
    }

    fn warm(&self, ctx: &mut ReqRespCtx) {
        let _ = self.predicate.test(ctx);
        let mut cel_ctx = ctx.cel.new_ctx(self);
        let _ = self.message_builder.eval(ctx, &mut cel_ctx);
    }
}

impl Task for DynamicTask {
    fn id(&self) -> &str {
        &self.task_id
    }

    fn dependencies(&self) -> &[String] {
        &self.dependencies
    }

    fn is_guard(&self) -> bool {
        self.is_guard
    }

    fn cel_types(&self) -> Vec<cel::StructDef> {
        (|| -> Result<Vec<cel::StructDef>, Box<dyn std::error::Error>> {
            let input_desc = self.service.input_descriptor()?;
            let output_desc = self.service.output_descriptor()?;
            let mut types = DescriptorConverter::collect_struct_defs(&input_desc)?;
            types.extend(DescriptorConverter::collect_struct_defs(&output_desc)?);
            Ok(types)
        })()
        .unwrap_or_else(|e| {
            error!("Failed to collect CEL types: {}", e);
            vec![]
        })
    }

    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        match self.predicate.test(ctx) {
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

            let mut cel_ctx = ctx.cel.new_ctx(&*self);
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
        let on_reply = self.on_reply;
        let is_guard = self.is_guard;

        if is_guard {
            ctx.barrier.raise();
        }

        TaskOutcome::Deferred {
            token_id,
            pending: Box::new(PendingTask::new(
                task_id.clone(),
                Box::new(move |ctx| {
                    let outcome = process_dynamic_response(
                        ctx, &service, &task_id, token_id, &name, on_reply,
                    );
                    if is_guard {
                        ctx.barrier.lower();
                    }
                    outcome
                }),
                is_guard,
            )),
        }
    }
}

fn process_dynamic_response(
    ctx: &mut ReqRespCtx,
    _service: &DynamicService,
    task_id: &str,
    token_id: u32,
    _name: &str,
    on_reply: Vec<Box<dyn Task>>,
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

    let (status_code, _response_size) = match ctx.get_grpc_response_data() {
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
        debug!("No onReply tasks, completing");
        return TaskOutcome::Done;
    }

    // todo(@adam-cattermole): Add response variable binding here with ctx.cel.add_scoped_binding()
    TaskOutcome::Requeued(on_reply)
}
