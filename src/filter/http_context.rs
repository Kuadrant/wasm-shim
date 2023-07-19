use crate::configuration::{
    Condition, DataItem, DataType, FilterConfig, PatternExpression, RateLimitPolicy,
};
use crate::envoy::{
    RateLimitDescriptor, RateLimitDescriptor_Entry, RateLimitRequest, RateLimitResponse,
    RateLimitResponse_Code,
};
use crate::utils::request_process_failure;
use log::{debug, info, warn};
use protobuf::Message;
use proxy_wasm::traits::{Context, HttpContext};
use proxy_wasm::types::Action;
use std::rc::Rc;
use std::time::Duration;

const RATELIMIT_SERVICE_NAME: &str = "envoy.service.ratelimit.v3.RateLimitService";
const RATELIMIT_METHOD_NAME: &str = "ShouldRateLimit";

pub struct Filter {
    pub context_id: u32,
    pub config: Rc<FilterConfig>,
    pub response_headers_to_add: Vec<(String, String)>,
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
            debug!("[context_id: {}] empty descriptors", self.context_id);
            return Action::Continue;
        }

        let mut rl_req = RateLimitRequest::new();
        rl_req.set_domain(rlp.domain.clone());
        rl_req.set_hits_addend(1);
        rl_req.set_descriptors(descriptors);

        let rl_req_serialized = Message::write_to_bytes(&rl_req).unwrap(); // TODO(rahulanand16nov): Error Handling

        match self.dispatch_grpc_call(
            rlp.service.as_str(),
            RATELIMIT_SERVICE_NAME,
            RATELIMIT_METHOD_NAME,
            Vec::new(),
            Some(&rl_req_serialized),
            Duration::from_secs(5),
        ) {
            Ok(call_id) => info!("Initiated gRPC call (id# {}) to Limitador", call_id),
            Err(e) => {
                warn!("gRPC call to Limitador failed! {:?}", e);
                request_process_failure(&self.config.failure_mode);
            }
        }
        Action::Pause
    }

    fn build_descriptors(
        &self,
        rlp: &RateLimitPolicy,
    ) -> protobuf::RepeatedField<RateLimitDescriptor> {
        //::protobuf::RepeatedField::default()
        rlp.rules
            .iter()
            .filter(|rule| self.filter_rule_by_conditions(&rule.conditions))
            // flatten the vec<vec<data> to vec<data>
            .flat_map(|rule| &rule.data)
            // WRONG: each rule generates one descriptor
            jdsjd
            .flat_map(|data| self.build_descriptor(data))
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
        let attribute_path = p_e.selector.split(".").collect();
        match self.get_property(attribute_path) {
            None => {
                debug!(
                    "[context_id: {}]: selector not found: {}",
                    self.context_id, p_e.selector
                );
                false
            }
            // TODO(eastizle): not all fields are strings
            // https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/advanced/attributes
            Some(attribute_bytes) => match String::from_utf8(attribute_bytes) {
                Err(e) => {
                    debug!(
                        "[context_id: {}]: failed to parse selector value: {}, error: {}",
                        self.context_id, p_e.selector, e
                    );
                    false
                }
                Ok(attribute_value) => p_e
                    .operator
                    .eval(p_e.value.as_str(), attribute_value.as_str()),
            },
        }
    }

    fn build_descriptor(&self, data: &DataItem) -> Option<RateLimitDescriptor> {
        let mut entries = ::protobuf::RepeatedField::default();

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

                let attribute_path = selector_item.selector.split(".").collect();

                // TODO(eastizle): not all fields are strings
                // https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/advanced/attributes
                match self.get_property(attribute_path) {
                    None => {
                        debug!(
                            "[context_id: {}]: selector not found: {}",
                            self.context_id, selector_item.selector
                        );
                        match &selector_item.default {
                            None => return None, // skipping descriptors
                            Some(default_value) => {
                                let mut descriptor_entry = RateLimitDescriptor_Entry::new();
                                descriptor_entry.set_key(descriptor_key);
                                descriptor_entry.set_value(default_value.to_owned());
                                entries.push(descriptor_entry);
                            }
                        }
                    }
                    Some(attribute_bytes) => match String::from_utf8(attribute_bytes) {
                        Err(e) => {
                            debug!(
                                "[context_id: {}]: failed to parse selector value: {}, error: {}",
                                self.context_id, selector_item.selector, e
                            );
                            return None;
                        }
                        Ok(attribute_value) => {
                            let mut descriptor_entry = RateLimitDescriptor_Entry::new();
                            descriptor_entry.set_key(descriptor_key);
                            descriptor_entry.set_value(attribute_value);
                            entries.push(descriptor_entry);
                        }
                    },
                }
            }
        }

        let mut res = RateLimitDescriptor::new();
        res.set_entries(entries);
        Some(res)
    }
}

impl HttpContext for Filter {
    fn on_http_request_headers(&mut self, _: usize, _: bool) -> Action {
        info!("on_http_request_headers #{}", self.context_id);

        match self
            .config
            .index
            .get_longest_match_policy(self.request_authority().as_str())
        {
            None => {
                info!(
                    "context #{}: Allowing request to pass because zero descriptors generated",
                    self.context_id
                );
                Action::Continue
            }
            Some(rlp) => self.process_rate_limit_policy(rlp),
        }
    }

    fn on_http_response_headers(&mut self, _num_headers: usize, _end_of_stream: bool) -> Action {
        for (name, value) in &self.response_headers_to_add {
            self.add_http_response_header(name, value);
        }
        Action::Continue
    }
}

impl Context for Filter {
    fn on_grpc_call_response(&mut self, token_id: u32, status_code: u32, resp_size: usize) {
        info!(
            "received gRPC call response: token: {}, status: {}",
            token_id, status_code
        );

        let res_body_bytes = match self.get_grpc_call_response_body(0, resp_size) {
            Some(bytes) => bytes,
            None => {
                warn!("grpc response body is empty!");
                request_process_failure(&self.config.failure_mode);
                return;
            }
        };

        let rl_resp: RateLimitResponse = match Message::parse_from_bytes(&res_body_bytes) {
            Ok(res) => res,
            Err(e) => {
                warn!("failed to parse grpc response body into RateLimitResponse message: {e}");
                request_process_failure(&self.config.failure_mode);
                return;
            }
        };

        match rl_resp {
            RateLimitResponse {
                overall_code: RateLimitResponse_Code::UNKNOWN,
                ..
            } => {
                request_process_failure(&self.config.failure_mode);
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
