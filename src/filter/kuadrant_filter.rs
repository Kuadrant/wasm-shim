use crate::kuadrant::{Pipeline, PipelineFactory, PipelineState, ReqRespCtx};
use crate::metrics::METRICS;
use proxy_wasm::traits::{Context, HttpContext};
use proxy_wasm::types::Action;
use std::ops::Not;
use std::rc::Rc;
use tracing::{debug, error, trace, warn};

pub struct KuadrantFilter {
    context_id: u32,
    factory: Rc<PipelineFactory>,
    pipeline: Option<Pipeline>,
    in_response_phase: bool,
    force_resume: bool,
}

impl KuadrantFilter {
    pub fn new(context_id: u32, factory: Rc<PipelineFactory>) -> Self {
        Self {
            context_id,
            factory,
            pipeline: None,
            in_response_phase: false,
            force_resume: false,
        }
    }

    fn should_pause(&self) -> bool {
        self.pipeline.as_ref().is_some_and(|p| p.requires_pause())
    }

    #[allow(clippy::expect_used)]
    fn should_resume(&self) -> bool {
        let pipeline = self.pipeline.as_ref().expect("pipeline must be present");

        if pipeline.is_terminated() && self.in_response_phase {
            return self.should_pause().not();
        }

        pipeline.is_terminated().not() && self.should_pause().not()
    }
}

impl Context for KuadrantFilter {
    fn on_grpc_call_response(&mut self, token_id: u32, status_code: u32, response_size: usize) {
        debug!(
            "#{} on_grpc_call_response: received gRPC call response: token: {}, status: {}",
            self.context_id, token_id, status_code
        );
        if let Some(pipeline) = self.pipeline.take() {
            let should_resume = match pipeline.digest(token_id, status_code, response_size) {
                PipelineState::InProgress(p) => {
                    self.pipeline = Some(*p);
                    self.should_resume()
                }
                PipelineState::Completed { should_resume } => {
                    self.pipeline = None;
                    trace!(
                        "#{} PipelineState::Completed: should_resume={}",
                        self.context_id,
                        should_resume
                    );
                    should_resume || self.force_resume || self.in_response_phase
                }
            };

            if should_resume {
                let result = if self.in_response_phase {
                    trace!("on_grpc_call_response: resume_http_response");
                    self.resume_http_response()
                } else {
                    trace!("on_grpc_call_response: resume_http_request");
                    self.resume_http_request()
                };

                if let Err(e) = result {
                    error!(
                        "#{} failed to resume filter processing: {:?}",
                        self.context_id, e
                    );
                }
            }
        } else {
            warn!("#{} received response without a pipeline", self.context_id);
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
                match pipeline.eval() {
                    PipelineState::InProgress(p) => {
                        self.pipeline = Some(*p);
                    }
                    PipelineState::Completed { .. } => {
                        self.pipeline = None;
                    }
                }
                if self.should_pause() {
                    trace!("on_http_request_headers: pause");
                    Action::Pause
                } else {
                    trace!("on_http_request_headers: continue");
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

    fn on_http_request_body(&mut self, buffer_size: usize, end_of_stream: bool) -> Action {
        debug!("#{} on_http_request_body", self.context_id);
        if let Some(mut pipeline) = self.pipeline.take() {
            pipeline
                .ctx
                .set_current_request_body_buffer_size(buffer_size, end_of_stream);
            match pipeline.eval() {
                PipelineState::InProgress(p) => {
                    self.pipeline = Some(*p);
                }
                PipelineState::Completed { .. } => {
                    self.pipeline = None;
                }
            }
        }
        if self.should_pause() {
            trace!("on_http_request_body: pause");
            Action::Pause
        } else {
            trace!("on_http_request_body: continue");
            Action::Continue
        }
    }

    fn on_http_response_headers(&mut self, _num_headers: usize, _end_of_stream: bool) -> Action {
        debug!("#{} on_http_response_headers", self.context_id);
        METRICS.allowed().increment();
        self.in_response_phase = true;
        if let Some(pipeline) = self.pipeline.take() {
            match pipeline.eval() {
                PipelineState::InProgress(p) => {
                    self.pipeline = Some(*p);
                }
                PipelineState::Completed { .. } => {
                    self.pipeline = None;
                }
            }
        }
        if self.should_pause() {
            trace!("on_http_response_headers: pause");
            Action::Pause
        } else {
            trace!("on_http_response_headers: continue");
            Action::Continue
        }
    }

    fn on_http_response_body(&mut self, buffer_size: usize, end_of_stream: bool) -> Action {
        debug!("#{} on_http_response_body", self.context_id);
        if let Some(mut pipeline) = self.pipeline.take() {
            pipeline
                .ctx
                .set_current_response_body_buffer_size(buffer_size, end_of_stream);
            match pipeline.eval() {
                PipelineState::InProgress(p) => {
                    self.pipeline = Some(*p);
                }
                PipelineState::Completed { .. } => {
                    self.pipeline = None;
                }
            }
        }
        if self.should_pause() {
            trace!("on_http_response_body: pause");
            Action::Pause
        } else {
            if self.pipeline.is_some() && end_of_stream {
                trace!("on_http_response_body: pipeline is some, pause");
                self.force_resume = true;
                Action::Pause
            } else {
                trace!("on_http_response_body: continue");
                Action::Continue
            }
        }
    }
}
