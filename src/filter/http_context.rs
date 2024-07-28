use crate::configuration::{
    Condition, DataItem, DataType, FailureMode, FilterConfig, PatternExpression, RateLimitPolicy,
    Rule,
};
use crate::envoy::{
    RateLimitDescriptor, RateLimitDescriptor_Entry, RateLimitRequest, RateLimitResponse,
    RateLimitResponse_Code,
};
use crate::filter::http_context::TracingHeader::{Baggage, Traceparent, Tracestate};
use log::{debug, error, warn};
use protobuf::Message;
use proxy_wasm::hostcalls;
use proxy_wasm::traits::{Context, HttpContext};
use proxy_wasm::types::{Action, Bytes, Status};
use std::rc::Rc;
use std::time::Duration;

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

pub struct HttpRateLimitFilter {
    pub context_id: u32,
    pub config: Rc<FilterConfig>,
    pub response_headers_to_add: Vec<(String, String)>,
    pub await_id: Option<u32>,
}

impl HttpRateLimitFilter {
    pub fn grpc_response(rl_resp: RateLimitResponse) -> Result<(bool, Vec<(String, String)>), ()> {
        match rl_resp {
            RateLimitResponse {
                overall_code: RateLimitResponse_Code::UNKNOWN,
                ..
            } => Err(()),
            RateLimitResponse {
                overall_code: RateLimitResponse_Code::OVER_LIMIT,
                response_headers_to_add: additional_headers,
                ..
            } => Ok((
                false,
                additional_headers
                    .into_iter()
                    .map(|f| (f.key, f.value))
                    .collect(),
            )),
            RateLimitResponse {
                overall_code: RateLimitResponse_Code::OK,
                response_headers_to_add: additional_headers,
                ..
            } => Ok((
                true,
                additional_headers
                    .into_iter()
                    .map(|f| (f.key, f.value))
                    .collect(),
            )),
        }
    }

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

    fn handle_error_on_grpc_response(&self) {
        match &self.config.failure_mode {
            FailureMode::Deny => {
                self.send_http_response(500, vec![], Some(b"Internal Server Error.\n"))
            }
            FailureMode::Allow => self.resume_http_request(),
        }
    }
}

impl Context for HttpRateLimitFilter {
    fn on_grpc_call_response(&mut self, token_id: u32, status_code: u32, resp_size: usize) {
        debug!(
            "#{} on_grpc_call_response: received gRPC call response: token: {token_id}, status: {status_code}",
            self.context_id
        );

        if Some(token_id) != self.await_id {
            error!(
                "Wrong token id from gRPC response, expected {:?}, got {}",
                self.await_id, token_id
            );
            self.handle_error_on_grpc_response();
            return;
        }

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

        match HttpRateLimitFilter::grpc_response(rl_resp) {
            Ok((allow, headers)) => {
                if allow {
                    self.response_headers_to_add = headers;
                } else {
                    self.send_http_response(
                        429,
                        headers
                            .iter()
                            .map(|(h, v)| (h.as_str(), v.as_str()))
                            .collect(),
                        Some(b"Too Many Requests\n"),
                    );
                }
            }
            Err(_) => {
                self.handle_error_on_grpc_response();
            }
        }
    }
}

impl HttpContext for HttpRateLimitFilter {
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
            Some(rlp) => {
                debug!("#{} ratelimitpolicy selected {}", self.context_id, rlp.name);

                let mut tracing_headers: Vec<(TracingHeader, Bytes)> = vec![];
                for header in TracingHeader::all() {
                    if let Some(value) = self.get_http_request_header_bytes(header.as_str()) {
                        tracing_headers.push((header, value))
                    }
                }

                let resolver = RateLimitPolicyResolver::new(self.context_id, rlp, tracing_headers);
                match resolver.conditions(attr_value, grpc_call) {
                    Ok(wait_for_response) => match wait_for_response {
                        None => Action::Continue,
                        Some(id) => {
                            self.await_id = Some(id);
                            Action::Pause
                        }
                    },
                    Err(_) => Action::Continue,
                }
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
struct RateLimitPolicyResolver<'a> {
    context_id: u32,
    tracing_headers: Vec<(TracingHeader, Bytes)>,
    rlp: &'a RateLimitPolicy,
}

type GrpcCall = fn(
    &str,
    initial_metadata: Vec<(&str, &[u8])>,
    RateLimitRequest,
) -> Result<u32, Status>;

impl<'a> RateLimitPolicyResolver<'a> {
    fn new(
        context_id: u32,
        rlp: &'a RateLimitPolicy,
        tracing_headers: Vec<(TracingHeader, Bytes)>,
    ) -> Self {
        Self {
            context_id,
            tracing_headers,
            rlp,
        }
    }

    pub fn conditions(
        self,
        attr_value: fn(Vec<&str>) -> Option<Bytes>,
        grpc_call: GrpcCall,
    ) -> Result<Option<u32>, String> {
        let descriptors = self.build_descriptors(self.rlp, attr_value);
        if descriptors.is_empty() {
            debug!(
                "#{} process_rate_limit_policy: empty descriptors",
                self.context_id
            );
            return Ok(None);
        }

        let mut rl_req = RateLimitRequest::new();
        rl_req.set_domain(self.rlp.domain.clone());
        rl_req.set_hits_addend(1);
        rl_req.set_descriptors(descriptors);

        let rl_tracing_headers = self
            .tracing_headers
            .iter()
            .map(|(header, value)| (header.as_str(), value.as_slice()))
            .collect();

        match grpc_call(self.rlp.service.as_str(), rl_tracing_headers, rl_req) {
            Ok(call_id) => {
                debug!(
                    "#{} initiated gRPC call (id# {}) to Limitador",
                    self.context_id, call_id
                );
                Ok(Some(call_id))
            }
            Err(e) => Err(format!("gRPC call to Limitador failed! {e:?}")),
        }
    }

    fn build_descriptors(
        &self,
        rlp: &RateLimitPolicy,
        attr_value: fn(Vec<&str>) -> Option<Bytes>,
    ) -> protobuf::RepeatedField<RateLimitDescriptor> {
        rlp.rules
            .iter()
            .filter(|rule: &&Rule| self.filter_rule_by_conditions(&rule.conditions, attr_value))
            // Mapping 1 Rule -> 1 Descriptor
            // Filter out empty descriptors
            .filter_map(|rule| self.build_single_descriptor(&rule.data, attr_value))
            .collect()
    }

    fn filter_rule_by_conditions(
        &self,
        conditions: &[Condition],
        attr_value: fn(Vec<&str>) -> Option<Bytes>,
    ) -> bool {
        if conditions.is_empty() {
            // no conditions is equivalent to matching all the requests.
            return true;
        }

        conditions
            .iter()
            .any(|condition| self.condition_applies(condition, attr_value))
    }

    fn condition_applies(
        &self,
        condition: &Condition,
        attr_value: fn(Vec<&str>) -> Option<Bytes>,
    ) -> bool {
        condition.all_of.iter().all(|pattern_expression| {
            self.pattern_expression_applies(pattern_expression, attr_value)
        })
    }

    fn pattern_expression_applies(
        &self,
        p_e: &PatternExpression,
        attr_value: fn(Vec<&str>) -> Option<Bytes>,
    ) -> bool {
        let attribute_path = p_e.path();
        let attribute_value = match attr_value(attribute_path) {
            None => {
                debug!(
                    "#{} pattern_expression_applies:  selector not found: {}, defaulting to ``",
                    self.context_id, p_e.selector
                );
                b"".to_vec()
            }
            Some(attribute_bytes) => attribute_bytes,
        };
        match p_e.eval(attribute_value) {
            Err(e) => {
                debug!(
                    "#{} pattern_expression_applies failed: {}",
                    self.context_id, e
                );
                false
            }
            Ok(result) => result,
        }
    }

    fn build_single_descriptor(
        &self,
        data_list: &[DataItem],
        attr_value: fn(Vec<&str>) -> Option<Bytes>,
    ) -> Option<RateLimitDescriptor> {
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
                        None => selector_item.path().to_string(),
                        Some(key) => key.to_owned(),
                    };

                    let attribute_path = selector_item.path();
                    let value = match attr_value(attribute_path.tokens()) {
                        None => {
                            debug!(
                                "#{} build_single_descriptor: selector not found: {}",
                                self.context_id, attribute_path
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
                                    self.context_id, attribute_path, e
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
}

fn attr_value(path: Vec<&str>) -> Option<Bytes> {
    hostcalls::get_property(path).unwrap()
}

fn grpc_call(
    upstream_name: &str,
    initial_metadata: Vec<(&str, &[u8])>,
    message: RateLimitRequest,
) -> Result<u32, Status> {
    let rl_req_serialized = Message::write_to_bytes(&message).unwrap(); // TODO(rahulanand16nov): Error Handling
    hostcalls::dispatch_grpc_call(
        upstream_name,
        RATELIMIT_SERVICE_NAME,
        RATELIMIT_METHOD_NAME,
        initial_metadata,
        Some(&rl_req_serialized),
        Duration::from_secs(5),
    )
}

#[cfg(test)]
mod tests {
    use crate::configuration::RateLimitPolicy;
    use crate::envoy::{RateLimitRequest, RateLimitResponse};
    use crate::filter::http_context::{HttpRateLimitFilter, RateLimitPolicyResolver};
    use proxy_wasm::types::{Bytes, Status};

    #[test]
    fn test_api() {
        let rlp = RateLimitPolicy::new(
            "Foo".into(),
            "foo.com".into(),
            "service".into(),
            ["foo.com".into()].into(),
            [].into(),
        );
        let resolver = RateLimitPolicyResolver::new(1, &rlp, vec![]);
        // request header phase: ignore req (Action::Continue) or RL gRPC req (Action::Pause);
        match resolver.conditions(attr_value, grpc_call) {
            Ok(grpc) => {
                match grpc {
                    Some(_call_id) => {
                        // Wait on gRPC response;
                        match HttpRateLimitFilter::grpc_response(RateLimitResponse {
                            overall_code: Default::default(),
                            statuses: Default::default(),
                            response_headers_to_add: Default::default(),
                            request_headers_to_add: Default::default(),
                            raw_body: vec![],
                            dynamic_metadata: Default::default(),
                            quota: Default::default(),
                            unknown_fields: Default::default(),
                            cached_size: Default::default(),
                        }) {
                            Ok((_allow, _headers)) => {
                                // response header phase: Add headers if needed.
                            }
                            Err(_) => {
                                // deal with error
                            }
                        }
                    }
                    None => {
                        // Done!
                    }
                }
            }
            Err(_msg) => {
                // warn!("{_msg}");
                // if let FailureMode::Deny = failure_mode {
                //     self.send_http_response(500, vec![], Some(b"Internal Server Error.\n"))
                // }
            }
        }
    }

    fn attr_value(path: Vec<&str>) -> Option<Bytes> {
        Some(path[0].as_bytes().to_vec())
    }
    fn grpc_call(
        _upstream_name: &str,
        _initial_metadata: Vec<(&str, &[u8])>,
        _message: RateLimitRequest,
    ) -> Result<u32, Status> {
        Ok(1)
    }
}
