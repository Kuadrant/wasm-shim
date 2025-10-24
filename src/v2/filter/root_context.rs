use super::kuadrant_filter::KuadrantFilter;
use crate::v2::configuration::PluginConfiguration;
use crate::v2::kuadrant::PipelineFactory;
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
    pub pipeline_factory: Rc<PipelineFactory>,
}

impl FilterRoot {
    pub fn new(context_id: u32) -> Self {
        Self {
            context_id,
            pipeline_factory: Rc::new(PipelineFactory::default()),
        }
    }
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
        Some(Box::new(KuadrantFilter::new(
            context_id,
            Rc::clone(&self.pipeline_factory),
        )))
    }

    fn on_configure(&mut self, _config_size: usize) -> bool {
        info!("#{} on_configure", self.context_id);
        let configuration: Vec<u8> = match self.get_plugin_configuration() {
            Ok(cfg) => match cfg {
                Some(c) => c,
                None => return false,
            },
            Err(status) => {
                error!("#{} on_configure: {:?}", self.context_id, status);
                return false;
            }
        };
        match serde_json::from_slice::<PluginConfiguration>(&configuration) {
            Ok(config) => {
                info!("plugin config parsed: {:?}", config);
                match PipelineFactory::try_from(config) {
                    Ok(factory) => {
                        self.pipeline_factory = Rc::new(factory);
                    }
                    Err(err) => {
                        error!("failed to compile plugin config: {:?}", err);
                        return false;
                    }
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::configuration::PluginConfiguration;

    #[test]
    fn invalid_json_fails_to_parse() {
        let invalid_json = "{ invalid json }";
        let result = serde_json::from_slice::<PluginConfiguration>(invalid_json.as_bytes());
        assert!(result.is_err());
    }

    #[test]
    fn config_with_invalid_predicate_fails_factory_creation() {
        let config_str = serde_json::json!({
            "services": {
                "test-service": {
                    "type": "auth",
                    "endpoint": "test-cluster",
                    "failureMode": "deny",
                    "timeout": "5s"
                }
            },
            "actionSets": [{
                "name": "test-action-set",
                "routeRuleConditions": {
                    "hostnames": ["example.com"],
                    "predicates": ["invalid syntax !!!"]
                },
                "actions": []
            }]
        })
        .to_string();

        let config = serde_json::from_slice::<PluginConfiguration>(config_str.as_bytes()).unwrap();
        let result = PipelineFactory::try_from(config);
        assert!(result.is_err());
    }
}
