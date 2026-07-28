use crate::kuadrant::pipeline::tasks::{TeardownAction, TeardownOutcome};
use crate::kuadrant::ReqRespCtx;
use crate::services::TracingService;
use std::rc::Rc;
use tracing::{debug, warn};

pub struct ExportTracesTask {
    service: Rc<TracingService>,
}

impl ExportTracesTask {
    pub fn new(ctx: &mut ReqRespCtx, service: Rc<TracingService>) -> Self {
        ctx.enter_request_span();
        Self { service }
    }
}

impl TeardownAction for ExportTracesTask {
    fn execute(self: Box<Self>, ctx: &mut ReqRespCtx) -> TeardownOutcome {
        // End the request span so it gets added to the buffer
        ctx.end_request_span();

        let processor = crate::tracing::get_span_processor();
        let spans = processor.take_pending_spans();

        if spans.is_empty() {
            debug!("No spans to export");
            return TeardownOutcome::Done;
        }

        debug!("Exporting {} spans", spans.len());

        let token_id = match self.service.dispatch_export(ctx, &spans) {
            Ok(id) => id,
            Err(e) => {
                warn!("Failed to dispatch trace export: {:?}", e);
                return TeardownOutcome::Done;
            }
        };

        debug!("Trace export dispatched with token_id: {}", token_id);

        TeardownOutcome::Deferred(token_id)
    }
}
