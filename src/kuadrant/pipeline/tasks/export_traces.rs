use crate::kuadrant::pipeline::tasks::{PendingTask, Task, TaskOutcome};
use crate::kuadrant::ReqRespCtx;
use crate::services::{OpenTelemetryService, Service};
use log::{debug, warn};
use std::rc::Rc;

pub struct ExportTracesTask {
    task_id: String,
    service: Rc<OpenTelemetryService>,
}

impl ExportTracesTask {
    pub fn new(task_id: String, service: Rc<OpenTelemetryService>) -> Self {
        Self { task_id, service }
    }

    pub fn new_if_pending(service: Rc<OpenTelemetryService>) -> Option<Self> {
        let processor = crate::tracing::get_span_processor();

        if !processor.has_pending_spans() {
            return None;
        }

        let task_id = format!("export_traces_{}", processor.pending_count());
        Some(Self::new(task_id, service))
    }
}

impl Task for ExportTracesTask {
    fn id(&self) -> Option<String> {
        Some(self.task_id.clone())
    }

    fn pauses_filter(&self, _ctx: &ReqRespCtx) -> bool {
        false
    }

    fn dependencies(&self) -> &[String] {
        &[]
    }

    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        let processor = crate::tracing::get_span_processor();
        let spans = processor.take_pending_spans();

        if spans.is_empty() {
            debug!("No spans to export");
            return TaskOutcome::Done;
        }

        debug!("Exporting {} spans", spans.len());

        let token_id = match self.service.dispatch_export(ctx, &spans) {
            Ok(id) => id,
            Err(e) => {
                warn!("Failed to dispatch trace export: {:?}", e);
                return TaskOutcome::Done;
            }
        };

        debug!("Trace export dispatched with token_id: {}", token_id);

        TaskOutcome::Deferred {
            token_id,
            pending: Box::new(PendingTask {
                task_id: self.task_id,
                pauses_filter: true,
                process_response: Box::new(move |ctx| {
                    match ctx.get_grpc_response_data() {
                        Ok((status_code, _response_size)) => {
                            if status_code == 0 {
                                debug!("Trace export succeeded (token_id: {})", token_id);
                            } else {
                                warn!(
                                    "Trace export failed with status {} (token_id: {})",
                                    status_code, token_id
                                );
                            }
                        }
                        Err(e) => {
                            warn!("Failed to get trace export response: {:?}", e);
                        }
                    }
                    TaskOutcome::Done
                }),
            }),
        }
    }
}
