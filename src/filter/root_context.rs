use crate::configuration::{FilterConfig, PluginConfiguration};
use crate::filter::http_context::Filter;
use log::{info, warn};
use proxy_wasm::traits::{Context, HttpContext, RootContext};
use proxy_wasm::types::ContextType;
use std::rc::Rc;

pub struct FilterRoot {
    pub context_id: u32,
    pub config: Rc<FilterConfig>,
}

impl RootContext for FilterRoot {
    fn on_vm_start(&mut self, _vm_configuration_size: usize) -> bool {
        info!("root-context #{}: VM started", self.context_id);
        true
    }

    fn create_http_context(&self, context_id: u32) -> Option<Box<dyn HttpContext>> {
        info!("create_http_context #{}", context_id);
        Some(Box::new(Filter {
            context_id,
            config: Rc::clone(&self.config),
            response_headers_to_add: Vec::default(),
        }))
    }

    fn on_configure(&mut self, _config_size: usize) -> bool {
        info!("on_configure #{}", self.context_id);
        let configuration: Vec<u8> = match self.get_plugin_configuration() {
            Some(c) => c,
            None => return false,
        };
        match serde_json::from_slice::<PluginConfiguration>(&configuration) {
            Ok(config) => {
                info!("plugin config parsed: {:?}", config);
                self.config = Rc::new(FilterConfig::from(config));
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
