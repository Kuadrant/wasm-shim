use crate::configuration::{Configuration, FilterConfig, RateLimitPolicy, Rule};
use crate::envoy::{
    RLA_action_specifier, RateLimitDescriptor, RateLimitDescriptor_Entry, RateLimitRequest,
    RateLimitResponse, RateLimitResponse_Code,
};
use crate::utils::{match_headers, request_process_failure, subdomain_match};
use log::{debug, info, warn};
use protobuf::Message;
use proxy_wasm::traits::{Context, HttpContext};
use proxy_wasm::types::Action;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

const RATELIMIT_SERVICE_NAME: &str = "envoy.service.ratelimit.v3.RateLimitService";
const RATELIMIT_METHOD_NAME: &str = "ShouldRateLimit";

pub struct Filter {
    pub context_id: u32,
    pub config: Rc<FilterConfig>,
}

impl Filter {
    fn request_path(&self) -> String {
        match self.get_http_request_header(":path") {
            None => {
                warn!(":path header not found");
                String::new()
            }
            Some(path) => path,
        }
    }

    fn request_method(&self) -> String {
        match self.get_http_request_header(":method") {
            None => {
                warn!(":method header not found");
                String::new()
            }
            Some(method) => method,
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

    fn process_rate_limit_policy(&self, rlp: &RateLimitPolicy) -> Action {
        let descriptors = self.build_descriptors(rlp);
        if descriptors.len() == 0 {
            debug!("[context_id: {}] empty descriptors", self.context_id);
            return Action::Continue;
        }

        let mut rl_req = RateLimitRequest::new();
        rl_req.set_domain(rlp.rate_limit_domain.clone());
        rl_req.set_hits_addend(1);
        rl_req.set_descriptors(descriptors);

        let rl_req_serialized = Message::write_to_bytes(&rl_req).unwrap(); // TODO(rahulanand16nov): Error Handling

        match self.dispatch_grpc_call(
            rlp.upstream_cluster.as_str(),
            RATELIMIT_SERVICE_NAME,
            RATELIMIT_METHOD_NAME,
            Vec::new(),
            Some(&rl_req_serialized),
            Duration::from_secs(5),
        ) {
            Ok(call_id) => info!("Initiated gRPC call (id# {}) to Limitador", call_id),
            Err(e) => {
                warn!("gRPC call to Limitador failed! {:?}", e);
                request_process_failure(self.config.failure_mode_deny);
            }
        }
        Action::Pause
    }

    fn build_descriptors(
        &self,
        rlp: &RateLimitPolicy,
    ) -> ::protobuf::RepeatedField<RateLimitDescriptor> {
        //::protobuf::RepeatedField::default()
        rlp.gateway_actions
            .iter()
            .filter(|ga| self.filter_configurations_by_rules(&ga.rules))
            // flatten the vec<vec<Configurations> to vec<Configuration>
            .flat_map(|ga| &ga.configurations)
            // All actions cannot be flatten! each vec of actions defines one potential descriptor
            .flat_map(|configuration| self.build_descriptor(configuration))
            .collect()
    }

    fn filter_configurations_by_rules(&self, rules: &Vec<Rule>) -> bool {
        if rules.is_empty() {
            // no rules is equivalent to matching all the requests.
            return true;
        }

        rules.iter().any(|rule| self.rule_applies(rule))
    }

    fn rule_applies(&self, rule: &Rule) -> bool {
        if !rule.paths.is_empty() {
            if !rule.paths.iter().any(|path| self.request_path().eq(path)) {
                return false;
            }
        }

        if !rule.methods.is_empty() {
            if !rule
                .methods
                .iter()
                .any(|method| self.request_method().eq(method))
            {
                return false;
            }
        }

        if !rule.hosts.is_empty() {
            if !rule
                .hosts
                .iter()
                .any(|subdomain| subdomain_match(subdomain, self.request_authority().as_str()))
            {
                return false;
            }
        }

        true
    }

    fn build_descriptor(&self, configuration: &Configuration) -> Option<RateLimitDescriptor> {
        let mut entries = ::protobuf::RepeatedField::default();
        for action in configuration.actions.iter() {
            let mut descriptor_entry = RateLimitDescriptor_Entry::new();
            match action {
                RLA_action_specifier::source_cluster(_) => {
                    match self.get_property(vec!["connection", "requested_server_name"]) {
                        None => {
                            debug!("requested service name not found");
                            return None;
                        }
                        Some(src_cluster_bytes) => {
                            match String::from_utf8(src_cluster_bytes) {
                                // NOTE(rahulanand16nov): not sure if it's correct.
                                Ok(src_cluster) => {
                                    descriptor_entry.set_key("source_cluster".into());
                                    descriptor_entry.set_value(src_cluster);
                                    entries.push(descriptor_entry);
                                }
                                Err(e) => {
                                    warn!("source_cluster action parsing error! {:?}", e);
                                    return None;
                                }
                            }
                        }
                    }
                }
                RLA_action_specifier::destination_cluster(_) => {
                    match self.get_property(vec!["cluster_name"]) {
                        None => {
                            debug!("cluster name not found");
                            return None;
                        }
                        Some(cluster_name_bytes) => match String::from_utf8(cluster_name_bytes) {
                            Ok(cluster_name) => {
                                descriptor_entry.set_key("destination_cluster".into());
                                descriptor_entry.set_value(cluster_name);
                                entries.push(descriptor_entry);
                            }
                            Err(e) => {
                                warn!("cluster_name action parsing error! {:?}", e);
                                return None;
                            }
                        },
                    }
                }
                RLA_action_specifier::request_headers(rh) => {
                    match self.get_http_request_header(rh.get_header_name()) {
                        None => {
                            debug!("header name {} not found", rh.get_header_name());
                            return None;
                        }
                        Some(header_value) => {
                            descriptor_entry.set_key(rh.get_descriptor_key().into());
                            descriptor_entry.set_value(header_value);
                            entries.push(descriptor_entry);
                        }
                    }
                }
                RLA_action_specifier::remote_address(_) => {
                    match self.get_http_request_header("x-forwarded-for") {
                        None => {
                            debug!("x-forwarded-for header not found");
                            return None;
                        }
                        Some(remote_addess) => {
                            descriptor_entry.set_key("remote_address".into());
                            descriptor_entry.set_value(remote_addess);
                            entries.push(descriptor_entry);
                        }
                    }
                }
                RLA_action_specifier::generic_key(gk) => {
                    descriptor_entry.set_key(gk.get_descriptor_key().into());
                    descriptor_entry.set_value(gk.get_descriptor_value().into());
                    entries.push(descriptor_entry);
                }
                RLA_action_specifier::header_value_match(hvm) => {
                    let request_headers: HashMap<_, _> =
                        self.get_http_request_headers().into_iter().collect();

                    if hvm.get_expect_match().get_value()
                        == match_headers(&request_headers, hvm.get_headers())
                    {
                        descriptor_entry.set_key("header_match".into());
                        descriptor_entry.set_value(hvm.get_descriptor_value().into());
                        entries.push(descriptor_entry);
                    } else {
                        debug!("header_value_match does not add entry");
                        return None;
                    }
                }
                RLA_action_specifier::dynamic_metadata(_) => todo!(),
                RLA_action_specifier::metadata(_) => todo!(),
                RLA_action_specifier::extension(_) => todo!(),
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
                request_process_failure(self.config.failure_mode_deny);
                return;
            }
        };

        let rl_resp: RateLimitResponse = match Message::parse_from_bytes(&res_body_bytes) {
            Ok(res) => res,
            Err(e) => {
                warn!(
                    "failed to parse grpc response body into RateLimitResponse message: {}",
                    e
                );
                request_process_failure(self.config.failure_mode_deny);
                return;
            }
        };

        match rl_resp.get_overall_code() {
            RateLimitResponse_Code::UNKNOWN => {
                request_process_failure(self.config.failure_mode_deny);
                return;
            }
            RateLimitResponse_Code::OVER_LIMIT => {
                self.send_http_response(429, vec![], Some(b"Too Many Requests\n"));
                return;
            }
            RateLimitResponse_Code::OK => {}
        }
        self.resume_http_request();
    }
}
