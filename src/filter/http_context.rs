use crate::configuration::action_set::ActionSet;
use crate::configuration::{FailureMode, FilterConfig};
use crate::operation_dispatcher::{OperationDispatcher, OperationError};
use crate::service::GrpcService;
use log::{debug, warn};
use proxy_wasm::traits::{Context, HttpContext};
use proxy_wasm::types::Action;
use std::cell::RefCell;
use std::rc::Rc;

pub struct Filter {
    pub context_id: u32,
    pub config: Rc<FilterConfig>,
    pub response_headers_to_add: Vec<(String, String)>,
    pub operation_dispatcher: RefCell<OperationDispatcher>,
}

impl Filter {
    fn request_authority(&self) -> String {
        match self.get_http_request_header(":authority") {
            None => {
                warn!(":authority header not found");
                String::new()
            }
            Some(host) => {
                let split_host = host.split(':').collect::<Vec<_>>();
                split_host[0].to_owned()
            }
        }
    }

    #[allow(unknown_lints, clippy::manual_inspect)]
    fn process_action_sets(&self, action_sets: &[Rc<ActionSet>]) -> Action {
        if let Some(action_set) = action_sets
            .iter()
            .find(|action_set| action_set.conditions_apply())
        {
            debug!(
                "#{} action_set selected {}",
                self.context_id, action_set.name
            );
            if let Err(op_err) = self
                .operation_dispatcher
                .borrow_mut()
                .build_operations(&action_set.actions)
            {
                self.send_http_response(500, vec![], Some(format!("{op_err}").as_ref()));
            }
        } else {
            debug!(
                "#{} process_action_sets: no action_set with conditions applies",
                self.context_id
            );
            return Action::Continue;
        }

        match self.operation_dispatcher.borrow_mut().next() {
            Ok(Some(op)) => match op.get_result() {
                Ok(call_id) => {
                    debug!("#{} initiated gRPC call (id# {})", self.context_id, call_id);
                    Action::Pause
                }
                Err(e) => {
                    warn!("gRPC call failed! {e:?}");
                    if let FailureMode::Deny = op.get_failure_mode() {
                        self.send_http_response(500, vec![], Some(b"Internal Server Error.\n"))
                    }
                    Action::Continue
                }
            },
            Ok(None) => {
                Action::Continue // No operations left to perform
            }
            Err(OperationError {
                failure_mode: FailureMode::Deny,
                ..
            }) => {
                self.send_http_response(500, vec![], Some(b"Internal Server Error.\n"));
                Action::Continue
            }
            _ => Action::Continue,
        }
    }
}

impl HttpContext for Filter {
    fn on_http_request_headers(&mut self, _: usize, _: bool) -> Action {
        debug!("#{} on_http_request_headers", self.context_id);

        match self
            .config
            .index
            .get_longest_match_action_sets(self.request_authority().as_str())
        {
            None => {
                debug!(
                    "#{} allowing request to pass because zero descriptors generated",
                    self.context_id
                );
                Action::Continue
            }
            Some(action_sets) => self.process_action_sets(action_sets),
        }
    }

    fn on_http_response_headers(&mut self, _num_headers: usize, _end_of_stream: bool) -> Action {
        debug!("#{} on_http_response_headers", self.context_id);
        for (name, value) in &self.response_headers_to_add {
            self.add_http_response_header(name, value);
        }
        Action::Continue
    }

    fn on_log(&mut self) {
        debug!("#{} completed.", self.context_id);
    }
}

impl Context for Filter {
    fn on_grpc_call_response(&mut self, token_id: u32, status_code: u32, resp_size: usize) {
        debug!(
            "#{} on_grpc_call_response: received gRPC call response: token: {token_id}, status: {status_code}",
            self.context_id
        );

        let op_res = self
            .operation_dispatcher
            .borrow()
            .get_waiting_operation(token_id);

        match op_res {
            Ok(operation) => {
                if GrpcService::process_grpc_response(operation, resp_size).is_ok() {
                    // call the next op
                    match self.operation_dispatcher.borrow_mut().next() {
                        Ok(_) => {} // no action needed
                        Err(op_err) => {
                            // If desired, we could check the error status.
                            GrpcService::handle_error_on_grpc_response(op_err.failure_mode);
                        }
                    }
                    self.resume_http_request();
                }
            }
            Err(e) => {
                warn!("No Operation found with token_id: {token_id}");
                GrpcService::handle_error_on_grpc_response(e.failure_mode);
            }
        }
    }
}
