use super::kuadrant_filter::KuadrantFilter;
use super::DescriptorManager;
use crate::configuration::PluginConfiguration;
use crate::kuadrant::PipelineFactory;
use crate::metrics::METRICS;
use crate::{WASM_SHIM_FEATURES, WASM_SHIM_GIT_HASH, WASM_SHIM_PROFILE, WASM_SHIM_VERSION};
use const_format::formatcp;
use proxy_wasm::traits::{Context, HttpContext, RootContext};
use proxy_wasm::types::ContextType;
use std::rc::Rc;
use std::time::Duration;
use tracing::{debug, error, info};

const WASM_SHIM_HEADER: &str = "Kuadrant wasm module";

pub struct FilterRoot {
    pub context_id: u32,
    pub pipeline_factory: Rc<PipelineFactory>,
    pub descriptor_manager: Rc<DescriptorManager>,
    tick_enabled: bool,
}

impl FilterRoot {
    pub fn new(context_id: u32) -> Self {
        Self {
            context_id,
            pipeline_factory: Rc::new(PipelineFactory::default()),
            descriptor_manager: Rc::new(DescriptorManager::default()),
            tick_enabled: false,
        }
    }

    fn set_tick_enabled(&mut self, enable: bool) {
        if enable && !self.tick_enabled {
            if let Err(e) = self.set_tick_period(self.descriptor_manager.tick_period()) {
                error!("Failed to enable tick: {:?}", e);
            } else {
                self.tick_enabled = true;
            }
        } else if !enable && self.tick_enabled {
            if let Err(e) = self.set_tick_period(Duration::ZERO) {
                error!("Failed to disable tick: {:?}", e);
            } else {
                self.tick_enabled = false;
            }
        }
    }

    fn process_config(&mut self, config: PluginConfiguration) -> bool {
        let descriptor_service = config.descriptor_service.clone();

        let factory = match PipelineFactory::try_from(config, &self.descriptor_manager) {
            Ok(f) => f,
            Err(err) => {
                error!("failed to compile plugin config: {:?}", err);
                return false;
            }
        };

        self.pipeline_factory = Rc::new(factory);
        self.descriptor_manager
            .set_descriptor_service(&descriptor_service);

        let has_dynamic_services = self.descriptor_manager.has_expected();
        if has_dynamic_services {
            if let Err(e) = self.descriptor_manager.fetch_missing(self) {
                error!("Failed to fetch descriptors: {}", e);
            }
        }

        self.set_tick_enabled(has_dynamic_services);

        true
    }

    fn handle_descriptor_response(
        &mut self,
        token_id: u32,
        status_code: u32,
        response_size: usize,
    ) -> Result<(), String> {
        if status_code != 0 {
            return Err(format!("descriptor fetch returned status {}", status_code));
        }

        let response_bytes = self
            .get_grpc_call_response_body(0, response_size)
            .map_err(|status| format!("could not get descriptor response: {:?}", status))?
            .ok_or_else(|| "descriptor response body is empty".to_string())?;

        self.descriptor_manager
            .handle_response(token_id, response_bytes)
    }
}

impl RootContext for FilterRoot {
    fn on_vm_start(&mut self, _vm_configuration_size: usize) -> bool {
        let full_version: &'static str = formatcp!(
            "v{WASM_SHIM_VERSION} ({WASM_SHIM_GIT_HASH}) {WASM_SHIM_FEATURES} {WASM_SHIM_PROFILE}"
        );

        opentelemetry::global::set_text_map_propagator(
            opentelemetry::propagation::TextMapCompositePropagator::new(vec![
                Box::new(opentelemetry_sdk::propagation::TraceContextPropagator::new()),
                Box::new(opentelemetry_sdk::propagation::BaggagePropagator::new()),
            ]),
        );

        log::info!(
            "#{} {} {}: VM started",
            self.context_id,
            WASM_SHIM_HEADER,
            full_version
        );
        true
    }

    fn create_http_context(&self, context_id: u32) -> Option<Box<dyn HttpContext>> {
        crate::tracing::update_log_level();
        debug!("#{} create_http_context", context_id);
        Some(Box::new(KuadrantFilter::new(
            context_id,
            Rc::clone(&self.pipeline_factory),
        )))
    }

    fn on_configure(&mut self, _config_size: usize) -> bool {
        log::info!("#{} on_configure", self.context_id);
        METRICS.configs().increment();
        let configuration: Vec<u8> = match self.get_plugin_configuration() {
            Ok(cfg) => match cfg {
                Some(c) => c,
                None => return false,
            },
            Err(status) => {
                log::error!("#{} on_configure: {:?}", self.context_id, status);
                return false;
            }
        };
        match serde_json::from_slice::<PluginConfiguration>(&configuration) {
            Ok(config) => {
                let use_tracing_exporter = config.observability.tracing.is_some();
                crate::tracing::init_observability(
                    use_tracing_exporter,
                    config.observability.default_level.as_deref(),
                );

                info!("plugin config parsed: {:?}", config);
                self.process_config(config)
            }
            Err(e) => {
                log::error!("failed to parse plugin config: {}", e);
                false
            }
        }
    }

    fn get_type(&self) -> Option<ContextType> {
        Some(ContextType::HttpContext)
    }

    fn on_tick(&mut self) {
        if let Err(e) = self.descriptor_manager.fetch_missing(self) {
            error!("Failed to fetch missing descriptors on tick: {}", e);
        }
    }
}

impl Context for FilterRoot {
    fn on_grpc_call_response(&mut self, token_id: u32, status_code: u32, response_size: usize) {
        if let Err(e) = self.handle_descriptor_response(token_id, status_code, response_size) {
            error!("Failed to handle descriptor response: {}", e);
        }
        self.descriptor_manager.reset_pending(token_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configuration::PluginConfiguration;
    use crate::filter::DescriptorKey;
    use prost_reflect::DescriptorPool;
    use prost_types::{
        DescriptorProto, FileDescriptorProto, FileDescriptorSet, MethodDescriptorProto,
        ServiceDescriptorProto,
    };

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
        let descriptor_manager = Rc::new(DescriptorManager::default());
        let result = PipelineFactory::try_from(config, &descriptor_manager);
        assert!(result.is_err());
    }

    #[test]
    fn test_factory_initialization_with_descriptors() {
        let file_descriptor = FileDescriptorProto {
            name: Some("test.proto".to_string()),
            package: Some("test".to_string()),
            message_type: vec![
                DescriptorProto {
                    name: Some("Request".to_string()),
                    ..Default::default()
                },
                DescriptorProto {
                    name: Some("Response".to_string()),
                    ..Default::default()
                },
            ],
            service: vec![ServiceDescriptorProto {
                name: Some("TestService".to_string()),
                method: vec![MethodDescriptorProto {
                    name: Some("TestMethod".to_string()),
                    input_type: Some(".test.Request".to_string()),
                    output_type: Some(".test.Response".to_string()),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        let fds = FileDescriptorSet {
            file: vec![file_descriptor],
        };

        let pool = DescriptorPool::from_file_descriptor_set(fds)
            .expect("Failed to create descriptor pool");

        let descriptor_manager = Rc::new(DescriptorManager::default());
        descriptor_manager.insert_pool(
            DescriptorKey::new("test-cluster".to_string(), "test.TestService".to_string()),
            pool,
        );

        let config_str = serde_json::json!({
            "services": {
                "dynamic-service": {
                    "type": "dynamic",
                    "endpoint": "test-cluster",
                    "failureMode": "deny",
                    "timeout": "1s",
                    "grpcService": "test.TestService",
                    "grpcMethod": "TestMethod"
                }
            },
            "actionSets": []
        })
        .to_string();

        let config = serde_json::from_slice::<PluginConfiguration>(config_str.as_bytes()).unwrap();
        let result = PipelineFactory::try_from(config, &descriptor_manager);
        assert!(result.is_ok());
    }

    #[test]
    fn test_factory_succeeds_without_descriptors() {
        let descriptor_manager = Rc::new(DescriptorManager::default());

        let config_str = serde_json::json!({
            "services": {
                "dynamic-service": {
                    "type": "dynamic",
                    "endpoint": "test-cluster",
                    "failureMode": "deny",
                    "timeout": "1s",
                    "grpcService": "test.TestService",
                    "grpcMethod": "TestMethod"
                }
            },
            "actionSets": []
        })
        .to_string();

        let config = serde_json::from_slice::<PluginConfiguration>(config_str.as_bytes()).unwrap();
        let result = PipelineFactory::try_from(config, &descriptor_manager);
        assert!(result.is_ok());
    }
}
