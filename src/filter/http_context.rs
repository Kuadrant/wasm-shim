use crate::configuration::{FailureMode, FilterConfig};
use crate::envoy::{RateLimitResponse, RateLimitResponse_Code};
use crate::filter::http_context::TracingHeader::{Baggage, Traceparent, Tracestate};
use crate::policy::Policy;
use crate::service::rate_limit::RateLimitService;
use crate::service::Service;
use log::{debug, warn};
use protobuf::Message;
use proxy_wasm::traits::{Context, HttpContext};
use proxy_wasm::types::{Action, Bytes};
use std::rc::Rc;

// tracing headers
pub enum TracingHeader {
    Traceparent,
    Tracestate,
    Baggage,
}

impl TracingHeader {
    fn all() -> [Self; 3] {
        [Traceparent, Tracestate, Baggage]
    }

    fn as_str(&self) -> &'static str {
        match self {
            Traceparent => "traceparent",
            Tracestate => "tracestate",
            Baggage => "baggage",
        }
    }
}

pub struct Filter {
    pub context_id: u32,
    pub config: Rc<FilterConfig>,
    pub response_headers_to_add: Vec<(String, String)>,
    pub tracing_headers: Vec<(TracingHeader, Bytes)>,
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

    fn process_rate_limit_policy(&self, rlp: &Policy) -> Action {
        let descriptors = rlp.build_descriptors(self);
        if descriptors.is_empty() {
            debug!(
                "#{} process_rate_limit_policy: empty descriptors",
                self.context_id
            );
            return Action::Continue;
        }
        let rl_tracing_headers = self
            .tracing_headers
            .iter()
            .map(|(header, value)| (header.as_str(), value.as_slice()))
            .collect();

        let rls = RateLimitService::new(rlp.service.as_str(), rl_tracing_headers);
        let message = RateLimitService::message(rlp.domain.clone(), descriptors);

        match rls.send(message) {
            Ok(call_id) => {
                debug!(
                    "#{} initiated gRPC call (id# {}) to Limitador",
                    self.context_id, call_id
                );
                Action::Pause
            }
            Err(e) => {
                warn!("gRPC call to Limitador failed! {e:?}");
                if let FailureMode::Deny = self.config.failure_mode {
                    self.send_http_response(500, vec![], Some(b"Internal Server Error.\n"))
                }
                Action::Continue
            }
        }
    }

    fn handle_error_on_grpc_response(&self) {
        match &self.config.failure_mode {
            FailureMode::Deny => {
                self.send_http_response(500, vec![], Some(b"Internal Server Error.\n"))
            }
            FailureMode::Allow => self.resume_http_request(),
        }
    }
}

impl HttpContext for Filter {
    fn on_http_request_headers(&mut self, _: usize, _: bool) -> Action {
        debug!("#{} on_http_request_headers", self.context_id);

        for header in TracingHeader::all() {
            if let Some(value) = self.get_http_request_header_bytes(header.as_str()) {
                self.tracing_headers.push((header, value))
            }
        }

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
            Some(rlp) => {
                debug!("#{} ratelimitpolicy selected {}", self.context_id, rlp.name);
                self.process_rate_limit_policy(rlp)
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

        let res_body_bytes = match self.get_grpc_call_response_body(0, resp_size) {
            Some(bytes) => bytes,
            None => {
                warn!("grpc response body is empty!");
                self.handle_error_on_grpc_response();
                return;
            }
        };

        let rl_resp: RateLimitResponse = match Message::parse_from_bytes(&res_body_bytes) {
            Ok(res) => res,
            Err(e) => {
                warn!("failed to parse grpc response body into RateLimitResponse message: {e}");
                self.handle_error_on_grpc_response();
                return;
            }
        };

        match rl_resp {
            RateLimitResponse {
                overall_code: RateLimitResponse_Code::UNKNOWN,
                ..
            } => {
                self.handle_error_on_grpc_response();
                return;
            }
            RateLimitResponse {
                overall_code: RateLimitResponse_Code::OVER_LIMIT,
                response_headers_to_add: rl_headers,
                ..
            } => {
                let mut response_headers = vec![];
                for header in &rl_headers {
                    response_headers.push((header.get_key(), header.get_value()));
                }
                self.send_http_response(429, response_headers, Some(b"Too Many Requests\n"));
                return;
            }
            RateLimitResponse {
                overall_code: RateLimitResponse_Code::OK,
                response_headers_to_add: additional_headers,
                ..
            } => {
                for header in additional_headers {
                    self.response_headers_to_add
                        .push((header.key, header.value));
                }
            }
        }
        self.resume_http_request();
    }
}
