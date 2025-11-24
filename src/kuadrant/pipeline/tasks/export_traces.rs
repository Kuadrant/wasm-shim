use crate::kuadrant::pipeline::tasks::{PendingTask, Task, TaskOutcome};
use crate::kuadrant::ReqRespCtx;
use crate::services::OpenTelemetryService;
use log::{debug, warn};
use std::rc::Rc;

pub struct ExportTracesTask {
    task_id: String,
    service: Rc<OpenTelemetryService>,
    dependencies: Vec<String>,
}

impl ExportTracesTask {
    pub fn new(service: Rc<OpenTelemetryService>, dependencies: Vec<String>) -> Self {
        crate::tracing::init_tracing();

        Self {
            task_id: "export_traces".to_string(),
            service,
            dependencies,
        }
    }
}

impl Task for ExportTracesTask {
    fn id(&self) -> Option<String> {
        Some(self.task_id.clone())
    }

    fn pauses_filter(&self) -> bool {
        false
    }

    fn dependencies(&self) -> &[String] {
        &self.dependencies
    }

    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        // End the request span so it gets added to the buffer
        ctx.end_request_span();

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
