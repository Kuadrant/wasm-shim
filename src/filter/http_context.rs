use crate::configuration::{
    Condition, DataItem, DataType, FailureMode, FilterConfig, PatternExpression, RateLimitPolicy,
    Rule,
};
use crate::envoy::{
    RateLimitDescriptor, RateLimitDescriptor_Entry, RateLimitRequest, RateLimitResponse,
    RateLimitResponse_Code,
};
use crate::filter::http_context::TracingHeader::{Baggage, Traceparent, Tracestate};
use crate::utils::tokenize_with_escaping;
use log::{debug, warn};
use protobuf::Message;
use proxy_wasm::traits::{Context, HttpContext};
use proxy_wasm::types::{Action, Bytes};
use std::rc::Rc;
use std::time::Duration;
use crate::envoy::properties::EnvoyTypeMapper;

const RATELIMIT_SERVICE_NAME: &str = "envoy.service.ratelimit.v3.RateLimitService";
const RATELIMIT_METHOD_NAME: &str = "ShouldRateLimit";

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
    pub property_mapper: Rc<EnvoyTypeMapper>,
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

    fn process_rate_limit_policy(&self, rlp: &RateLimitPolicy) -> Action {
        let descriptors = self.build_descriptors(rlp);
        if descriptors.is_empty() {
            debug!(
                "#{} process_rate_limit_policy: empty descriptors",
                self.context_id
            );
            return Action::Continue;
        }

        let mut rl_req = RateLimitRequest::new();
        rl_req.set_domain(rlp.domain.clone());
        rl_req.set_hits_addend(1);
        rl_req.set_descriptors(descriptors);

        let rl_req_serialized = Message::write_to_bytes(&rl_req).unwrap(); // TODO(rahulanand16nov): Error Handling

        let rl_tracing_headers = self
            .tracing_headers
            .iter()
            .map(|(header, value)| (header.as_str(), value.as_slice()))
            .collect();

        match self.dispatch_grpc_call(
            rlp.service.as_str(),
            RATELIMIT_SERVICE_NAME,
            RATELIMIT_METHOD_NAME,
            rl_tracing_headers,
            Some(&rl_req_serialized),
            Duration::from_secs(5),
        ) {
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

    fn build_descriptors(
        &self,
        rlp: &RateLimitPolicy,
    ) -> protobuf::RepeatedField<RateLimitDescriptor> {
        rlp.rules
            .iter()
            .filter(|rule: &&Rule| self.filter_rule_by_conditions(&rule.conditions))
            // Mapping 1 Rule -> 1 Descriptor
            // Filter out empty descriptors
            .filter_map(|rule| self.build_single_descriptor(&rule.data))
            .collect()
    }

    fn filter_rule_by_conditions(&self, conditions: &[Condition]) -> bool {
        if conditions.is_empty() {
            // no conditions is equivalent to matching all the requests.
            return true;
        }

        conditions
            .iter()
            .any(|condition| self.condition_applies(condition))
    }

    fn condition_applies(&self, condition: &Condition) -> bool {
        condition
            .all_of
            .iter()
            .all(|pattern_expression| self.pattern_expression_applies(pattern_expression))
    }

    fn pattern_expression_applies(&self, p_e: &PatternExpression) -> bool {
        let attribute_path = tokenize_with_escaping(&p_e.selector, '.', '\\');
        // convert a Vec<String> to Vec<&str>
        // attribute_path.iter().map(AsRef::as_ref).collect()
        let attribute_value = match self
            .get_property(attribute_path.iter().map(AsRef::as_ref).collect())
        {
            None => {
                debug!(
                    "#{} pattern_expression_applies: selector not found: {}, defaulting to ``",
                    self.context_id, p_e.selector
                );
                "".to_string()
            }
            // TODO(eastizle): not all fields are strings
            // https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/advanced/attributes
            Some(attribute_bytes) => match String::from_utf8(attribute_bytes) {
                Err(e) => {
                    debug!(
                            "#{} pattern_expression_applies: failed to parse selector value: {}, error: {}",
                            self.context_id, p_e.selector, e
                        );
                    return false;
                }
                Ok(attribute_value) => attribute_value,
            },
        };
        p_e.operator
            .eval(p_e.value.as_str(), attribute_value.as_str())
    }

    fn build_single_descriptor(&self, data_list: &[DataItem]) -> Option<RateLimitDescriptor> {
        let mut entries = ::protobuf::RepeatedField::default();

        // iterate over data items to allow any data item to skip the entire descriptor
        for data in data_list.iter() {
            match &data.item {
                DataType::Static(static_item) => {
                    let mut descriptor_entry = RateLimitDescriptor_Entry::new();
                    descriptor_entry.set_key(static_item.key.to_owned());
                    descriptor_entry.set_value(static_item.value.to_owned());
                    entries.push(descriptor_entry);
                }
                DataType::Selector(selector_item) => {
                    let descriptor_key = match &selector_item.key {
                        None => selector_item.selector.to_owned(),
                        Some(key) => key.to_owned(),
                    };

                    let attribute_path = tokenize_with_escaping(&selector_item.selector, '.', '\\');
                    // convert a Vec<String> to Vec<&str>
                    // attribute_path.iter().map(AsRef::as_ref).collect()
                    let value = match self
                        .get_property(attribute_path.iter().map(AsRef::as_ref).collect())
                    {
                        None => {
                            debug!(
                                "#{} build_single_descriptor: selector not found: {}",
                                self.context_id, selector_item.selector
                            );
                            match &selector_item.default {
                                None => return None, // skipping the entire descriptor
                                Some(default_value) => default_value.clone(),
                            }
                        }
                        // TODO(eastizle): not all fields are strings
                        // https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/advanced/attributes
                        Some(attribute_bytes) => match String::from_utf8(attribute_bytes) {
                            Err(e) => {
                                debug!(
                                    "#{} build_single_descriptor: failed to parse selector value: {}, error: {}",
                                    self.context_id, selector_item.selector, e
                                );
                                return None;
                            }
                            Ok(attribute_value) => attribute_value,
                        },
                    };
                    let mut descriptor_entry = RateLimitDescriptor_Entry::new();
                    descriptor_entry.set_key(descriptor_key);
                    descriptor_entry.set_value(value);
                    entries.push(descriptor_entry);
                }
            }
        }

        let mut res = RateLimitDescriptor::new();
        res.set_entries(entries);
        Some(res)
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
