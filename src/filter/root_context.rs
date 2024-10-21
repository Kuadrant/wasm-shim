use crate::configuration::{FilterConfig, PluginConfiguration, ServiceType};
use crate::filter::http_context::Filter;
use crate::operation_dispatcher::OperationDispatcher;
use crate::service::{GrpcServiceHandler, HeaderResolver, ServiceMetrics};
use const_format::formatcp;
use log::{debug, error, info};
use proxy_wasm::hostcalls;
use proxy_wasm::traits::{Context, HttpContext, RootContext};
use proxy_wasm::types::{ContextType, MetricType};
use std::rc::Rc;

const WASM_SHIM_VERSION: &str = env!("CARGO_PKG_VERSION");
const WASM_SHIM_PROFILE: &str = env!("WASM_SHIM_PROFILE");
const WASM_SHIM_FEATURES: &str = env!("WASM_SHIM_FEATURES");
const WASM_SHIM_GIT_HASH: &str = env!("WASM_SHIM_GIT_HASH");
const WASM_SHIM_HEADER: &str = "Kuadrant wasm module";

pub struct FilterRoot {
    pub context_id: u32,
    pub config: Rc<RuntimeConfig>,
    pub rate_limit_ok_metric_id: u32,
    pub rate_limit_error_metric_id: u32,
    pub rate_limit_over_limit_metric_id: u32,
    pub rate_limit_failure_mode_allowed_metric_id: u32,
    pub auth_ok_metric_id: u32,
    pub auth_error_metric_id: u32,
    pub auth_denied_metric_id: u32,
    pub auth_failure_mode_allowed_metric_id: u32,
}

impl RootContext for FilterRoot {
    fn on_vm_start(&mut self, _vm_configuration_size: usize) -> bool {
        let full_version: &'static str = formatcp!(
            "v{WASM_SHIM_VERSION} ({WASM_SHIM_GIT_HASH}) {WASM_SHIM_FEATURES} {WASM_SHIM_PROFILE}"
        );

        info!(
            "#{} {} {}: VM started",
            self.context_id, WASM_SHIM_HEADER, full_version
        );

        self.rate_limit_ok_metric_id =
            match hostcalls::define_metric(MetricType::Counter, "kuadrant.rate_limit.ok") {
                Ok(metric_id) => metric_id,
                Err(e) => panic!("Error: {:?}", e),
            };
        self.rate_limit_error_metric_id =
            match hostcalls::define_metric(MetricType::Counter, "kuadrant.rate_limit.error") {
                Ok(metric_id) => metric_id,
                Err(e) => panic!("Error: {:?}", e),
            };
        self.rate_limit_over_limit_metric_id =
            match hostcalls::define_metric(MetricType::Counter, "kuadrant.rate_limit.over_limit") {
                Ok(metric_id) => metric_id,
                Err(e) => panic!("Error: {:?}", e),
            };
        self.rate_limit_failure_mode_allowed_metric_id = match hostcalls::define_metric(
            MetricType::Counter,
            "kuadrant.rate_limit.failure_mode_allowed",
        ) {
            Ok(metric_id) => metric_id,
            Err(e) => panic!("Error: {:?}", e),
        };
        self.auth_ok_metric_id =
            match hostcalls::define_metric(MetricType::Counter, "kuadrant.auth.ok") {
                Ok(metric_id) => metric_id,
                Err(e) => panic!("Error: {:?}", e),
            };
        self.auth_error_metric_id =
            match hostcalls::define_metric(MetricType::Counter, "kuadrant.auth.error") {
                Ok(metric_id) => metric_id,
                Err(e) => panic!("Error: {:?}", e),
            };
        self.auth_denied_metric_id =
            match hostcalls::define_metric(MetricType::Counter, "kuadrant.auth.denied") {
                Ok(metric_id) => metric_id,
                Err(e) => panic!("Error: {:?}", e),
            };
        self.auth_failure_mode_allowed_metric_id = match hostcalls::define_metric(
            MetricType::Counter,
            "kuadrant.auth.failure_mode_allowed",
        ) {
            Ok(metric_id) => metric_id,
            Err(e) => panic!("Error: {:?}", e),
        };
        true
    }

    fn create_http_context(&self, context_id: u32) -> Option<Box<dyn HttpContext>> {
        debug!("#{} create_http_context", context_id);
        let header_resolver = Rc::new(HeaderResolver::new());
        Some(Box::new(Filter {
            context_id,
            config: Rc::clone(&self.config),
            response_headers_to_add: Vec::default(),
            operation_dispatcher: OperationDispatcher::new(header_resolver).into(),
        }))
    }

    fn on_configure(&mut self, _config_size: usize) -> bool {
        info!("#{} on_configure", self.context_id);
        let configuration: Vec<u8> = match self.get_plugin_configuration() {
            Some(c) => c,
            None => return false,
        };
        match serde_json::from_slice::<PluginConfiguration>(&configuration) {
            Ok(config) => {
                info!("plugin config parsed: {:?}", config);
                let runtime_config =
                    match <PluginConfiguration as TryInto<RuntimeConfig>>::try_into(config) {
                        Ok(cfg) => cfg,
                        Err(err) => {
                            error!("failed to compile plugin config: {}", err);
                            return false;
                        }
                    };
                self.config = Rc::new(runtime_config);
            }
            Err(e) => {
                error!("failed to parse plugin config: {}", e);
                return false;
            }
        }
        true
    }

    fn get_type(&self) -> Option<ContextType> {
        Some(ContextType::HttpContext)
    }
}

impl Context for FilterRoot {}

impl FilterRoot {
    fn service_metrics(&self, service_type: &ServiceType) -> ServiceMetrics {
        match service_type {
            ServiceType::Auth => ServiceMetrics::new(
                self.auth_ok_metric_id,
                self.auth_error_metric_id,
                self.auth_denied_metric_id,
                self.auth_failure_mode_allowed_metric_id,
            ),
            ServiceType::RateLimit => ServiceMetrics::new(
                self.rate_limit_ok_metric_id,
                self.rate_limit_error_metric_id,
                self.rate_limit_over_limit_metric_id,
                self.rate_limit_failure_mode_allowed_metric_id,
            ),
        }
    }
}
