use crate::envoy_ext_auth::{AttributeContext, CheckRequest, CheckResponse};
use crate::utils::{request_process_failure, set_attribute_peer, set_attribute_request, UtilsErr};
use log::{info, warn};
use protobuf::Message;
use proxy_wasm::traits::{Context, HttpContext, RootContext};
use proxy_wasm::types::{Action, ContextType, LogLevel};
use serde::Deserialize;
use std::time::Duration;

const AUTHORIZATION_SERVICE_NAME: &str = "envoy.service.auth.v3.Authorization";
const AUTHORIZATION_METHOD_NAME: &str = "Check";

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

struct Filter {
    context_id: u32,
    config: FilterConfig,
}

impl Filter {
    fn config(&self) -> &FilterConfig {
        &self.config
    }

    fn context_id(&self) -> u32 {
        self.context_id
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
        let metadata_bytes = self.get_property(vec!["metadata"]);
        if metadata_bytes.is_some() {
            let metadata = Message::parse_from_bytes(&metadata_bytes.unwrap())?;
            attr_context.set_metadata_context(metadata);
        }
        check_request.set_attributes(attr_context);
        Ok(check_request)
    }
}

#[derive(Deserialize, Debug, Clone)]
struct FilterConfig {
    // Upstream Authorino's service name.
    auth_cluster: String,
    // Deny request when faced with an irrecoverable failure.
    failure_mode_deny: bool,
}

impl FilterConfig {
    pub fn new() -> Self {
        Self {
            auth_cluster: String::new(),
            failure_mode_deny: true,
        }
    }
    fn auth_cluster(&self) -> &str {
        self.auth_cluster.as_ref()
    }

    fn failure_mode_deny(&self) -> bool {
        self.failure_mode_deny
    }
}

struct FilterRoot {
    context_id: u32,
    config: FilterConfig,
}

impl FilterRoot {
    fn config(&self) -> &FilterConfig {
        &self.config
    }
}

impl RootContext for FilterRoot {
    fn on_vm_start(&mut self, _vm_configuration_size: usize) -> bool {
        info!("root-context #{}: VM started", self.context_id);
        true
    }

    fn create_http_context(&self, context_id: u32) -> Option<Box<dyn HttpContext>> {
        Some(Box::new(Filter {
            context_id,
            config: self.config().clone(),
        }))
    }

    fn on_configure(&mut self, _config_size: usize) -> bool {
        let configuration: Vec<u8> = match self.get_configuration() {
            Some(c) => c,
            None => return false,
        };
        match serde_json::from_slice::<FilterConfig>(configuration.as_ref()) {
            Ok(config) => {
                info!("plugin config parsed: {:?}", config);
                self.config = config;
                true
            }
            Err(e) => {
                warn!("failed to parse plugin config: {}", e);
                false
            }
        }
    }

    fn get_type(&self) -> Option<ContextType> {
        Some(ContextType::HttpContext)
    }
}

impl HttpContext for Filter {
    fn on_http_request_headers(&mut self, _: usize) -> Action {
        let context_id = self.context_id();
        info!("context #{}: on_http_request_headers called", context_id);
        self.add_http_request_header("HTTP_FROM_WASM", "Yes, it's working!");

        let check_request = self.create_check_request().unwrap();
        let check_req_serialized = Message::write_to_bytes(&check_request).unwrap();

        match self.dispatch_grpc_call(
            self.config().auth_cluster(),
            AUTHORIZATION_SERVICE_NAME,
            AUTHORIZATION_METHOD_NAME,
            Vec::new(),
            Some(&check_req_serialized),
            Duration::from_secs(5),
        ) {
            Ok(_) => info!("GRPC CALL SUCCESS!"),
            Err(e) => warn!("GRPC CALL FAILED! {:?}", e),
        }
        Action::Pause
    }
}

impl Context for Filter {
    fn on_grpc_call_response(&mut self, token_id: u32, status_code: u32, resp_size: usize) {
        info!(
            "recieved authorization response: token: {}, status: {}",
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
        let check_response: CheckResponse = match Message::parse_from_bytes(&res_body_bytes) {
            Ok(res) => res,
            Err(e) => {
                warn!("failed to parse grpc response body: {}", e);
                request_process_failure(self.config().failure_mode_deny());
                return;
            }
        };

        if check_response.get_status().get_code() != 0 {
            self.send_http_response(403, vec![], Some(b"Access forbidden.\n"));
        }
        self.resume_http_request();
    }
}
impl Context for FilterRoot {}
