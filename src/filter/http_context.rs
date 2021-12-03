use crate::configuration::{FilterConfig, Operation};
use crate::envoy::{
    AttributeContext, CheckRequest, CheckResponse, RLA_action_specifier, RateLimitRequest,
    RateLimitResponse, RateLimitResponse_Code,
};
use crate::filter::root_context::FilterRoot;
use crate::utils::{
    descriptor_from_actions, request_process_failure, set_attribute_peer, set_attribute_request,
    UtilsErr,
};
use log::{info, warn};
use protobuf::Message;
use proxy_wasm::traits::{Context, HttpContext, RootContext};
use proxy_wasm::types::{Action, LogLevel};
use std::time::Duration;

const AUTHORIZATION_SERVICE_NAME: &str = "envoy.service.auth.v3.Authorization";
const AUTHORIZATION_METHOD_NAME: &str = "Check";

const RATELIMIT_SERVICE_NAME: &str = "envoy.service.ratelimit.v3.RateLimitService";
const RATELIMIT_METHOD_NAME: &str = "ShouldRateLimit";

#[no_mangle]
pub fn _start() {
    proxy_wasm::set_log_level(LogLevel::Trace);
    std::panic::set_hook(Box::new(|panic_info| {
        proxy_wasm::hostcalls::log(LogLevel::Critical, &panic_info.to_string()).unwrap();
    }));
    proxy_wasm::set_root_context(|context_id| -> Box<dyn RootContext> {
        Box::new(FilterRoot {
            context_id,
            config: FilterConfig::new(),
        })
    });
}

pub struct Filter {
    pub context_id: u32,
    pub config: FilterConfig,
}

impl Filter {
    fn config(&self) -> &FilterConfig {
        &self.config
    }

    fn context_id(&self) -> u32 {
        self.context_id
    }

    fn create_ratelimit_request(
        &self,
        domain: &str,
        actions: &[RLA_action_specifier],
    ) -> Result<RateLimitRequest, UtilsErr> {
        let mut rl_req = RateLimitRequest::new();
        rl_req.set_domain(domain.into());
        rl_req.set_hits_addend(1); // TODO(rahulanand16nov): take this from config as well.
        rl_req
            .mut_descriptors()
            .push(descriptor_from_actions(self, actions)?);
        Ok(rl_req)
    }

    fn create_check_request(&self) -> Result<CheckRequest, UtilsErr> {
        let mut check_request = CheckRequest::new();
        let mut attr_context = AttributeContext::new();
        let service_bytes = self
            .get_property(vec!["connection", "requested_server_name"])
            .unwrap_or_else(|| {
                warn!("requested service name not found");
                vec![]
            });
        let service = String::from_utf8(service_bytes)?;

        set_attribute_peer(self, attr_context.mut_source(), service, false)?;
        set_attribute_peer(self, attr_context.mut_destination(), String::new(), true)?;
        set_attribute_request(self, attr_context.mut_request())?;
        // Note: destination labels not available.
        // Note: context_extension may be taken from plugin config as well.
        if let Some(bytes) = self.get_property(vec!["metadata"]) {
            let metadata = Message::parse_from_bytes(&bytes)?;
            attr_context.set_metadata_context(metadata);
        }
        check_request.set_attributes(attr_context);
        Ok(check_request)
    }

    fn handle_next_operation(&mut self) {
        let ops = self.config.operations().last(); // NOTE: Reverse the array in the config!
        let url_path = String::from_utf8(
            self.get_property(vec!["request", "url_path"])
                .unwrap_or_else(|| {
                    warn!("request's URL path not found!");
                    vec![]
                }),
        )
        .unwrap_or_default();

        if let Some(ops) = ops {
            if ops.is_excluded(&url_path) {
                self.config.mut_operations().pop();
                self.handle_next_operation();
                return;
            }
            match ops {
                Operation::Authenticate(auth_config) => {
                    let check_request = self.create_check_request().unwrap(); // TODO(rahulanand16nov): Error Handling
                    let check_req_serialized = Message::write_to_bytes(&check_request).unwrap(); // TODO(rahulanand16nov): Error Handling

                    match self.dispatch_grpc_call(
                        auth_config.upstream_cluster(),
                        AUTHORIZATION_SERVICE_NAME,
                        AUTHORIZATION_METHOD_NAME,
                        Vec::new(),
                        Some(&check_req_serialized),
                        Duration::from_secs(5),
                    ) {
                        Ok(call_id) => info!("Initiated gRPC call (id# {}) to Authorino", call_id),
                        Err(e) => {
                            warn!("gRPC call to Authorino failed! {:?}", e);
                            request_process_failure(self.config().failure_mode_deny());
                        }
                    }
                }
                Operation::RateLimit(rl_config) => {
                    let rl_request = self
                        .create_ratelimit_request(rl_config.domain(), rl_config.actions())
                        .unwrap(); // TODO(rahulanand16nov): Error Handling
                    let rl_req_serialized = Message::write_to_bytes(&rl_request).unwrap(); // TODO(rahulanand16nov): Error Handling

                    match self.dispatch_grpc_call(
                        rl_config.upstream_cluster(),
                        RATELIMIT_SERVICE_NAME,
                        RATELIMIT_METHOD_NAME,
                        Vec::new(),
                        Some(&rl_req_serialized),
                        Duration::from_secs(5),
                    ) {
                        Ok(call_id) => info!("Initiated gRPC call (id# {}) to Limitador", call_id),
                        Err(e) => {
                            warn!("gRPC call to Limitador failed! {:?}", e);
                            request_process_failure(self.config().failure_mode_deny());
                        }
                    }
                }
            }
        } else {
            // No operations left, forward the request to the next filter.
            self.resume_http_request();
        }
    }
}

impl HttpContext for Filter {
    fn on_http_request_headers(&mut self, _: usize) -> Action {
        let context_id = self.context_id();
        info!("context #{}: on_http_request_headers called", context_id);

        self.handle_next_operation();

        Action::Pause
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
                request_process_failure(self.config().failure_mode_deny());
                return;
            }
        };

        let ops = match self.config.mut_operations().pop() {
            Some(ops) => ops,
            None => {
                // this can happen if ops is popped somewhere before resp is received.
                warn!("gRPC response received but no operations found!");
                request_process_failure(self.config().failure_mode_deny());
                return;
            }
        };

        match ops {
            Operation::Authenticate(_) => {
                let check_resp: CheckResponse = match Message::parse_from_bytes(&res_body_bytes) {
                    Ok(res) => res,
                    Err(e) => {
                        warn!(
                            "failed to parse grpc response body into CheckResponse message: {}",
                            e
                        );
                        request_process_failure(self.config().failure_mode_deny());
                        return;
                    }
                };

                if check_resp.get_status().get_code() != 0 {
                    self.send_http_response(403, vec![], Some(b"Access forbidden.\n"));
                    return;
                }
            }
            Operation::RateLimit(_) => {
                let rl_resp: RateLimitResponse = match Message::parse_from_bytes(&res_body_bytes) {
                    Ok(res) => res,
                    Err(e) => {
                        warn!(
                            "failed to parse grpc response body into RateLimitResponse message: {}",
                            e
                        );
                        request_process_failure(self.config().failure_mode_deny());
                        return;
                    }
                };

                match rl_resp.get_overall_code() {
                    RateLimitResponse_Code::UNKNOWN => {
                        request_process_failure(self.config().failure_mode_deny());
                        return;
                    },
                    RateLimitResponse_Code::OVER_LIMIT => {
                        self.send_http_response(429, vec![], Some(b"Too Many Requests\n"));
                        return;
                    },
                    RateLimitResponse_Code::OK => {}
                }
            }
        };

        if self.config.operations().is_empty() {
            self.resume_http_request();
        } else {
            self.handle_next_operation();
        }
    }
}
