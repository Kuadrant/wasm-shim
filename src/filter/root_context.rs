use crate::action_set_index::ActionSetIndex;
use crate::configuration::PluginConfiguration;
use crate::filter::kuadrant_filter::KuadrantFilter;
use crate::service::HeaderResolver;
use const_format::formatcp;
use log::{debug, error, info};
use proxy_wasm::traits::{Context, HttpContext, RootContext};
use proxy_wasm::types::ContextType;
use std::rc::Rc;

const WASM_SHIM_VERSION: &str = env!("CARGO_PKG_VERSION");
const WASM_SHIM_PROFILE: &str = env!("WASM_SHIM_PROFILE");
const WASM_SHIM_FEATURES: &str = env!("WASM_SHIM_FEATURES");
const WASM_SHIM_GIT_HASH: &str = env!("WASM_SHIM_GIT_HASH");
const WASM_SHIM_HEADER: &str = "Kuadrant wasm module";

pub struct FilterRoot {
    pub context_id: u32,
    pub action_set_index: Rc<ActionSetIndex>,
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
        true
    }

    fn create_http_context(&self, context_id: u32) -> Option<Box<dyn HttpContext>> {
        debug!("#{} create_http_context", context_id);
        let header_resolver = Rc::new(HeaderResolver::new());
        Some(Box::new(KuadrantFilter::new(
            context_id,
            Rc::clone(&self.action_set_index),
            header_resolver,
        )))
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
                let action_set_index =
                    match <PluginConfiguration as TryInto<ActionSetIndex>>::try_into(config) {
                        Ok(cfg) => cfg,
                        Err(err) => {
                            error!("failed to compile plugin config: {}", err);
                            return false;
                        }
                    };
                self.action_set_index = Rc::new(action_set_index);
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
