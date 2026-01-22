use crate::kuadrant::{Pipeline, PipelineFactory, ReqRespCtx};
use crate::metrics::METRICS;
use proxy_wasm::traits::{Context, HttpContext};
use proxy_wasm::types::Action;
use std::rc::Rc;
use tracing::{debug, error};

pub struct KuadrantFilter {
    context_id: u32,
    factory: Rc<PipelineFactory>,
    pipeline: Option<Pipeline>,
    in_response_phase: bool,
}

impl KuadrantFilter {
    pub fn new(context_id: u32, factory: Rc<PipelineFactory>) -> Self {
        Self {
            context_id,
            factory,
            pipeline: None,
            in_response_phase: false,
        }
    }

    fn should_pause(&self) -> bool {
        self.pipeline.as_ref().is_some_and(|p| p.requires_pause())
    }
}

impl Context for KuadrantFilter {
    fn on_grpc_call_response(&mut self, token_id: u32, status_code: u32, response_size: usize) {
        debug!(
            "#{} on_grpc_call_response: received gRPC call response: token: {}, status: {}",
            self.context_id, token_id, status_code
        );
        if let Some(pipeline) = self.pipeline.take() {
            self.pipeline = pipeline.digest(token_id, status_code, response_size);

            if !self.should_pause() {
                let result = if self.in_response_phase {
                    self.resume_http_response()
                } else {
                    self.resume_http_request()
                };

                if let Err(e) = result {
                    error!(
                        "#{} failed to resume filter processing: {:?}",
                        self.context_id, e
                    );
                }
            }
        }
    }
}

impl HttpContext for KuadrantFilter {
    fn on_http_request_headers(&mut self, _num_headers: usize, _end_of_stream: bool) -> Action {
        debug!("#{} on_http_request_headers", self.context_id);

        #[cfg(feature = "debug-host-behaviour")]
        crate::data::debug_all_well_known_attributes();

        let ctx = ReqRespCtx::default();

        match self.factory.build(ctx) {
            Ok(Some(pipeline)) => {
                debug!("#{} pipeline built successfully", self.context_id);
                METRICS.hits().increment();
                self.pipeline = pipeline.eval();
                if self.should_pause() {
                    Action::Pause
                } else {
                    Action::Continue
                }
            }
            Ok(None) => {
                debug!("#{} no matching route found", self.context_id);
                METRICS.misses().increment();
                Action::Continue
            }
            Err(e) => {
                error!("#{} failed to build pipeline: {:?}", self.context_id, e);
                METRICS.errors().increment();
                // todo(adam-cattermole): we should deny the request
                Action::Continue
            }
        }
    }

    fn on_http_request_body(&mut self, _buffer_size: usize, _end_of_stream: bool) -> Action {
        debug!("#{} on_http_request_body", self.context_id);
        if let Some(pipeline) = self.pipeline.take() {
            self.pipeline = pipeline.eval();
        }
        if self.should_pause() {
            Action::Pause
        } else {
            Action::Continue
        }
    }

    fn on_http_response_headers(&mut self, _num_headers: usize, _end_of_stream: bool) -> Action {
        debug!("#{} on_http_response_headers", self.context_id);
        METRICS.allowed().increment();
        self.in_response_phase = true;
        if let Some(pipeline) = self.pipeline.take() {
            self.pipeline = pipeline.eval();
        }
        if self.should_pause() {
            Action::Pause
        } else {
            Action::Continue
        }
    }

    fn on_http_response_body(&mut self, buffer_size: usize, end_of_stream: bool) -> Action {
        debug!("#{} on_http_response_body", self.context_id);
        if let Some(mut pipeline) = self.pipeline.take() {
            pipeline
                .ctx
                .set_current_response_body_buffer_size(buffer_size, end_of_stream);
            self.pipeline = pipeline.eval();
        }
        if self.should_pause() {
            Action::Pause
        } else {
            Action::Continue
        }
    }
}
