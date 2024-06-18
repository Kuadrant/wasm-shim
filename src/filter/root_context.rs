use crate::configuration::{FilterConfig, PluginConfiguration};
use crate::filter::http_context::Filter;
use const_format::formatcp;
use log::{debug, info, warn};
use proxy_wasm::traits::{Context, HttpContext, RootContext};
use proxy_wasm::types::ContextType;
use std::rc::Rc;
use crate::envoy::properties::EnvoyTypeMapper;

const WASM_SHIM_VERSION: &str = env!("CARGO_PKG_VERSION");
const WASM_SHIM_PROFILE: &str = env!("WASM_SHIM_PROFILE");
const WASM_SHIM_FEATURES: &str = env!("WASM_SHIM_FEATURES");
const WASM_SHIM_GIT_HASH: &str = env!("WASM_SHIM_GIT_HASH");
const WASM_SHIM_HEADER: &str = "Kuadrant wasm module";

pub struct FilterRoot {
    pub context_id: u32,
    pub config: Rc<FilterConfig>,
    pub property_mapper: Rc<EnvoyTypeMapper>,
}

impl RootContext for FilterRoot {
    fn on_vm_start(&mut self, _vm_configuration_size: usize) -> bool {
        let full_version: &'static str = formatcp!(
            "v{} ({}) {} {}",
            WASM_SHIM_VERSION,
            WASM_SHIM_GIT_HASH,
            WASM_SHIM_FEATURES,
            WASM_SHIM_PROFILE,
        );

        info!(
            "#{} {} {}: VM started",
            self.context_id, WASM_SHIM_HEADER, full_version
        );
        true
    }

    fn create_http_context(&self, context_id: u32) -> Option<Box<dyn HttpContext>> {
        debug!("#{} create_http_context", context_id);
        Some(Box::new(Filter {
            context_id,
            config: Rc::clone(&self.config),
            response_headers_to_add: Vec::default(),
            property_mapper: Rc::clone(&self.property_mapper),
            tracing_headers: Vec::default(),
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
                self.config = Rc::new(config.into());
            }
            Err(e) => {
                warn!("failed to parse plugin config: {}", e);
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
