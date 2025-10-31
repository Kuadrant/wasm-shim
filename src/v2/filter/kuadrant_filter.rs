use crate::v2::kuadrant::{Pipeline, PipelineFactory, ReqRespCtx};
use log::{debug, error};
use proxy_wasm::traits::{Context, HttpContext};
use proxy_wasm::types::Action;
use std::rc::Rc;

pub struct KuadrantFilter {
    context_id: u32,
    factory: Rc<PipelineFactory>,
    pipeline: Option<Pipeline>,
}

impl KuadrantFilter {
    pub fn new(context_id: u32, factory: Rc<PipelineFactory>) -> Self {
        Self {
            context_id,
            factory,
            pipeline: None,
        }
    }
}

impl Context for KuadrantFilter {
    fn on_grpc_call_response(&mut self, token_id: u32, status_code: u32, response_size: usize) {
        debug!("#{} on_grpc_call_response", self.context_id);
        if let Some(pipeline) = self.pipeline.take() {
            self.pipeline = pipeline.digest(token_id, status_code, response_size);
            // todo(adam-cattermole): Check pipeline.is_blocked() to determine Action::Pause vs Action::Continue
        }
    }
}

impl HttpContext for KuadrantFilter {
    fn on_http_request_headers(&mut self, _: usize, _: bool) -> Action {
        debug!("#{} on_http_request_headers", self.context_id);

        let ctx = ReqRespCtx::default();

        match self.factory.build(ctx) {
            Ok(Some(pipeline)) => {
                debug!("#{} pipeline built successfully", self.context_id);
                self.pipeline = pipeline.eval();
                // todo(adam-cattermole): Check pipeline.is_blocked() to determine Action::Pause vs Action::Continue
                Action::Continue
            }
            Ok(None) => {
                debug!("#{} no matching route found", self.context_id);
                Action::Continue
            }
            Err(e) => {
                error!("#{} failed to build pipeline: {:?}", self.context_id, e);
                // todo(adam-cattermole): we should deny the request
                Action::Continue
            }
        }
    }

    fn on_http_request_body(&mut self, _body_size: usize, _end_of_stream: bool) -> Action {
        debug!("#{} on_http_request_body", self.context_id);
        if let Some(pipeline) = self.pipeline.take() {
            self.pipeline = pipeline.eval();
            // todo(adam-cattermole): Check pipeline.is_blocked() to determine Action::Pause vs Action::Continue
        }
        Action::Continue
    }

    fn on_http_response_headers(&mut self, _: usize, _: bool) -> Action {
        debug!("#{} on_http_response_headers", self.context_id);
        if let Some(pipeline) = self.pipeline.take() {
            self.pipeline = pipeline.eval();
            // todo(adam-cattermole): Check pipeline.is_blocked() to determine Action::Pause vs Action::Continue
        }
        Action::Continue
    }

    fn on_http_response_body(&mut self, _body_size: usize, _end_of_stream: bool) -> Action {
        debug!("#{} on_http_response_body", self.context_id);
        if let Some(pipeline) = self.pipeline.take() {
            self.pipeline = pipeline.eval();
            // todo(adam-cattermole): Check pipeline.is_blocked() to determine Action::Pause vs Action::Continue
        }
        Action::Continue
    }
}
