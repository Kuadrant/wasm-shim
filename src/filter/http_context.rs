use crate::attribute::store_metadata;
use crate::configuration::{ExtensionType, FailureMode, FilterConfig};
use crate::envoy::{CheckResponse_oneof_http_response, RateLimitResponse, RateLimitResponse_Code};
use crate::operation_dispatcher::OperationDispatcher;
use crate::policy::Policy;
use crate::service::grpc_message::GrpcMessageResponse;
use log::{debug, warn};
use proxy_wasm::traits::{Context, HttpContext};
use proxy_wasm::types::Action;
use std::rc::Rc;

pub struct Filter {
    pub context_id: u32,
    pub config: Rc<FilterConfig>,
    pub response_headers_to_add: Vec<(String, String)>,
    pub operation_dispatcher: OperationDispatcher,
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

    fn process_policy(&self, policy: &Policy) -> Action {
        if let Some(rule) = policy.find_rule_that_applies() {
            self.operation_dispatcher.build_operations(rule);
        } else {
            debug!("#{} process_policy: no rule applied", self.context_id);
            return Action::Continue;
        }

        if let Some(operation) = self.operation_dispatcher.next() {
            match operation.get_result() {
                Ok(call_id) => {
                    debug!("#{} initiated gRPC call (id# {})", self.context_id, call_id);
                    Action::Pause
                }
                Err(e) => {
                    warn!("gRPC call failed! {e:?}");
                    if let FailureMode::Deny = operation.get_failure_mode() {
                        self.send_http_response(500, vec![], Some(b"Internal Server Error.\n"))
                    }
                    Action::Continue
                }
            }
        } else {
            Action::Continue
        }
    }

    fn handle_error_on_grpc_response(&self, failure_mode: &FailureMode) {
        match failure_mode {
            FailureMode::Deny => {
                self.send_http_response(500, vec![], Some(b"Internal Server Error.\n"))
            }
            FailureMode::Allow => self.resume_http_request(),
        }
    }

    fn process_ratelimit_grpc_response(
        &mut self,
        rl_resp: GrpcMessageResponse,
        failure_mode: &FailureMode,
    ) {
        match rl_resp {
            GrpcMessageResponse::RateLimit(RateLimitResponse {
                overall_code: RateLimitResponse_Code::UNKNOWN,
                ..
            }) => {
                self.handle_error_on_grpc_response(failure_mode);
            }
            GrpcMessageResponse::RateLimit(RateLimitResponse {
                overall_code: RateLimitResponse_Code::OVER_LIMIT,
                response_headers_to_add: rl_headers,
                ..
            }) => {
                let mut response_headers = vec![];
                for header in &rl_headers {
                    response_headers.push((header.get_key(), header.get_value()));
                }
                self.send_http_response(429, response_headers, Some(b"Too Many Requests\n"));
            }
            GrpcMessageResponse::RateLimit(RateLimitResponse {
                overall_code: RateLimitResponse_Code::OK,
                response_headers_to_add: additional_headers,
                ..
            }) => {
                for header in additional_headers {
                    self.response_headers_to_add
                        .push((header.key, header.value));
                }
            }
            _ => {}
        }
        self.operation_dispatcher.next();
    }

    fn process_auth_grpc_response(
        &mut self,
        auth_resp: GrpcMessageResponse,
        failure_mode: &FailureMode,
    ) {
        if let GrpcMessageResponse::Auth(check_response) = auth_resp {
            // store dynamic metadata in filter state
            store_metadata(check_response.get_dynamic_metadata());

            match check_response.http_response {
                Some(CheckResponse_oneof_http_response::ok_response(ok_response)) => {
                    debug!(
                        "#{} process_auth_grpc_response: received OkHttpResponse",
                        self.context_id
                    );

                    ok_response
                        .get_response_headers_to_add()
                        .iter()
                        .for_each(|header| {
                            self.add_http_response_header(
                                header.get_header().get_key(),
                                header.get_header().get_value(),
                            )
                        });
                }
                Some(CheckResponse_oneof_http_response::denied_response(denied_response)) => {
                    debug!(
                        "#{} process_auth_grpc_response: received DeniedHttpResponse",
                        self.context_id
                    );

                    let mut response_headers = vec![];
                    denied_response.get_headers().iter().for_each(|header| {
                        response_headers.push((
                            header.get_header().get_key(),
                            header.get_header().get_value(),
                        ))
                    });
                    self.send_http_response(
                        denied_response.get_status().code as u32,
                        response_headers,
                        Some(denied_response.get_body().as_ref()),
                    );
                    return;
                }
                None => {
                    self.handle_error_on_grpc_response(failure_mode);
                    return;
                }
            }
        }
        self.operation_dispatcher.next();
    }
}

impl HttpContext for Filter {
    fn on_http_request_headers(&mut self, _: usize, _: bool) -> Action {
        debug!("#{} on_http_request_headers", self.context_id);

        match self
            .config
            .index
            .get_longest_match_policy(self.request_authority().as_str())
        {
            None => {
                debug!(
                    "#{} allowing request to pass because zero descriptors generated",
                    self.context_id
                );
                Action::Continue
            }
            Some(policy) => {
                debug!("#{} policy selected {}", self.context_id, policy.name);
                self.process_policy(policy)
            }
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

        if let Some(operation) = self.operation_dispatcher.get_operation(token_id) {
            let failure_mode = &operation.get_failure_mode();
            let res_body_bytes = match self.get_grpc_call_response_body(0, resp_size) {
                Some(bytes) => bytes,
                None => {
                    warn!("grpc response body is empty!");
                    self.handle_error_on_grpc_response(failure_mode);
                    return;
                }
            };
            let res =
                match GrpcMessageResponse::new(operation.get_extension_type(), &res_body_bytes) {
                    Ok(res) => res,
                    Err(e) => {
                        warn!(
                        "failed to parse grpc response body into GrpcMessageResponse message: {e}"
                    );
                        self.handle_error_on_grpc_response(failure_mode);
                        return;
                    }
                };
            match operation.get_extension_type() {
                ExtensionType::Auth => self.process_auth_grpc_response(res, failure_mode),
                ExtensionType::RateLimit => self.process_ratelimit_grpc_response(res, failure_mode),
            }

            if let Some(_op) = self.operation_dispatcher.next() {
            } else {
                self.resume_http_request()
            }
        } else {
            warn!("No Operation found with token_id: {token_id}");
            self.handle_error_on_grpc_response(&FailureMode::Deny); // TODO(didierofrivia): Decide on what's the default failure mode
        }
    }
}
