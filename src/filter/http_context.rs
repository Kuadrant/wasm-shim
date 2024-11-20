use crate::configuration::FailureMode;
#[cfg(feature = "debug-host-behaviour")]
use crate::data;
use crate::operation_dispatcher::{OperationDispatcher, OperationError};
use crate::runtime_action_set::RuntimeActionSet;
use crate::runtime_config::RuntimeConfig;
use crate::service::GrpcService;
use log::{debug, warn};
use proxy_wasm::traits::{Context, HttpContext};
use proxy_wasm::types::Action;
use std::cell::RefCell;
use std::rc::Rc;

pub struct Filter {
    pub context_id: u32,
    pub config: Rc<RuntimeConfig>,
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
    fn process_action_sets(&self, m_set_list: &[Rc<RuntimeActionSet>]) -> Action {
        if let Some(m_set) = m_set_list.iter().find(|m_set| m_set.conditions_apply()) {
            debug!("#{} action_set selected {}", self.context_id, m_set.name);
            //debug!("#{} runtime action_set {:#?}", self.context_id, m_set);
            self.operation_dispatcher
                .borrow_mut()
                .build_operations(&m_set.runtime_actions)
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
                status,
            }) => {
                warn!("OperationError Status: {status:?}");
                self.send_http_response(500, vec![], Some(b"Internal Server Error.\n"));
                Action::Continue
            }
            Err(OperationError {
                failure_mode: FailureMode::Allow,
                status,
            }) => {
                warn!("OperationError Status: {status:?}");
                Action::Continue
            }
        }
    }
}

impl HttpContext for Filter {
    fn on_http_request_headers(&mut self, _: usize, _: bool) -> Action {
        debug!("#{} on_http_request_headers", self.context_id);

        #[cfg(feature = "debug-host-behaviour")]
        data::debug_all_well_known_attributes();

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
            Some(m_sets) => self.process_action_sets(m_sets),
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
                if let Ok(result) = GrpcService::process_grpc_response(operation, resp_size) {
                    // add the response headers
                    self.response_headers_to_add.extend(result.response_headers);
                    // call the next op
                    match self.operation_dispatcher.borrow_mut().next() {
                        Ok(some_op) => {
                            if some_op.is_none() {
                                // No more operations left in queue, resuming
                                self.resume_http_request();
                            }
                        }
                        Err(op_err) => {
                            // If desired, we could check the error status.
                            GrpcService::handle_error_on_grpc_response(op_err.failure_mode);
                        }
                    }
                }
            }
            Err(e) => {
                warn!("No Operation found with token_id: {token_id}");
                GrpcService::handle_error_on_grpc_response(e.failure_mode);
            }
        }
    }
}
