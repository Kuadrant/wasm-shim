use crate::configuration::PluginConfiguration;
use crate::data::{
    attribute::AttributeState,
    cel::{Predicate, PredicateVec},
    Expression,
};
use crate::kuadrant::pipeline::blueprint::{Blueprint, CompileError};
use crate::kuadrant::pipeline::executor::Pipeline;
use crate::kuadrant::pipeline::tasks::Task;
use crate::kuadrant::ReqRespCtx;
use crate::services::ServiceInstance;
use log::debug;
use radix_trie::Trie;
use std::collections::HashMap;
use std::fmt::Display;
use std::rc::Rc;
use std::sync::Arc;

type RequestData = ((String, String), Expression);

pub struct PipelineFactory {
    index: Trie<String, Vec<Rc<Blueprint>>>,
    request_data: Arc<Vec<RequestData>>,
}

#[derive(Debug)]
pub enum BuildError {
    DataPending(String),
    EvaluationError(String),
}

impl Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::DataPending(msg) => write!(f, "Data pending: {}", msg),
            BuildError::EvaluationError(msg) => write!(f, "Evaluation error: {}", msg),
        }
    }
}

impl TryFrom<PluginConfiguration> for PipelineFactory {
    type Error = CompileError;

    fn try_from(config: PluginConfiguration) -> Result<Self, Self::Error> {
        let services: HashMap<String, ServiceInstance> = config
            .services
            .iter()
            .map(|(name, service_config)| {
                let instance = ServiceInstance::try_from(service_config)
                    .map_err(|e| CompileError::ServiceCreationFailed(format!("{}", e)))?;
                Ok((name.clone(), instance))
            })
            .collect::<Result<_, CompileError>>()?;

        let mut index = Trie::new();
        for config_action_set in &config.action_sets {
            let blueprint = Rc::new(Blueprint::compile(config_action_set, &services)?);

            for hostname in &config_action_set.route_rule_conditions.hostnames {
                let key = reverse_subdomain(hostname);
                index.map_with_default(
                    key,
                    |blueprints| blueprints.push(Rc::clone(&blueprint)),
                    vec![Rc::clone(&blueprint)],
                );
            }
        }

        let mut request_data: Vec<((String, String), Expression)> = config
            .request_data
            .iter()
            .filter_map(|(k, v)| {
                Expression::new(v).ok().map(|expr| {
                    let (domain, field) = domain_and_field_name(k);
                    ((domain.to_string(), field.to_string()), expr)
                })
            })
            .collect();
        request_data.sort_by(|a, b| a.0.cmp(&b.0));

        Ok(Self {
            index,
            request_data: Arc::new(request_data),
        })
    }
}

impl Default for PipelineFactory {
    fn default() -> Self {
        Self {
            index: Trie::new(),
            request_data: Arc::new(Vec::new()),
        }
    }
}

impl PipelineFactory {
    pub fn build(&self, ctx: ReqRespCtx) -> Result<Option<Pipeline>, BuildError> {
        let ctx = ctx.with_request_data(Arc::clone(&self.request_data));

        let blueprint = match self.select_blueprint(&ctx)? {
            Some(bp) => bp,
            None => return Ok(None),
        };

        let tasks: Vec<Box<dyn Task>> = blueprint.to_tasks(&ctx);
        if tasks.is_empty() {
            return Ok(None);
        }

        Ok(Some(Pipeline::new(ctx).with_tasks(tasks)))
    }

    fn select_blueprint(&self, ctx: &ReqRespCtx) -> Result<Option<&Rc<Blueprint>>, BuildError> {
        let hostname = self.get_hostname(ctx)?;

        let candidates = match self.index.get_ancestor_value(&reverse_subdomain(&hostname)) {
            Some(blueprints) => blueprints,
            None => {
                debug!("No matching blueprint found for hostname: {}", hostname);
                return Ok(None);
            }
        };

        for blueprint in candidates {
            if self.route_predicates_match(&blueprint.route_predicates, &blueprint.name, ctx)? {
                debug!(
                    "Selected blueprint {} for hostname: {}",
                    blueprint.name, hostname
                );
                return Ok(Some(blueprint));
            }
        }

        debug!("No matching blueprint found for hostname: {}", hostname);
        Ok(None)
    }

    fn get_hostname(&self, ctx: &ReqRespCtx) -> Result<String, BuildError> {
        match ctx.get_attribute::<String>("request.host") {
            Ok(AttributeState::Available(Some(host))) => {
                let split_host = host.split_once(':').map_or(host.as_str(), |(h, _)| h);
                Ok(split_host.to_owned())
            }
            Ok(AttributeState::Available(None)) => Err(BuildError::EvaluationError(
                "hostname not found".to_string(),
            )),
            Ok(AttributeState::Pending) => Err(BuildError::DataPending("hostname".to_string())),
            Err(e) => Err(BuildError::EvaluationError(e.to_string())),
        }
    }

    fn route_predicates_match(
        &self,
        predicates: &Vec<Predicate>,
        blueprint_name: &str,
        ctx: &ReqRespCtx,
    ) -> Result<bool, BuildError> {
        match predicates.apply(ctx) {
            Ok(AttributeState::Available(result)) => Ok(result),
            Ok(AttributeState::Pending) => Err(BuildError::DataPending(format!(
                "route predicate: {}",
                blueprint_name
            ))),
            Err(e) => Err(BuildError::EvaluationError(format!(
                "route predicate evaluation failed: {}",
                e
            ))),
        }
    }
}

fn reverse_subdomain(subdomain: &str) -> String {
    let mut s = subdomain.to_string();
    s.push('.');
    if s.starts_with('*') {
        s.remove(0);
    } else {
        s.insert(0, '$');
    }
    s.chars().rev().collect()
}

fn domain_and_field_name(name: &str) -> (&str, &str) {
    let haystack = &name[..name
        .char_indices()
        .rfind(|(_, c)| c.is_alphabetic())
        .map(|(i, _)| i)
        .unwrap_or_default()];
    haystack
        .rfind('.')
        .map(|i| {
            if i == 0 || i == name.len() - 1 {
                ("", name)
            } else {
                (&name[..i], &name[i + 1..])
            }
        })
        .unwrap_or(("", name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configuration::{
        Action, ActionSet, FailureMode, RouteRuleConditions, Service, ServiceType, Timeout,
    };
    use crate::kuadrant::MockWasmHost;

    fn build_test_config(
        hostnames: Vec<String>,
        predicates: Vec<String>,
        service_name: &str,
    ) -> PluginConfiguration {
        let mut services = HashMap::new();
        services.insert(
            service_name.to_string(),
            Service {
                service_type: ServiceType::Auth,
                endpoint: "test-cluster".to_string(),
                failure_mode: FailureMode::Deny,
                timeout: Timeout::default(),
            },
        );

        PluginConfiguration {
            request_data: HashMap::new(),
            services,
            action_sets: vec![ActionSet {
                name: "test-action-set".to_string(),
                route_rule_conditions: RouteRuleConditions {
                    hostnames,
                    predicates,
                },
                actions: vec![Action {
                    service: service_name.to_string(),
                    scope: "test-scope".to_string(),
                    predicates: vec![],
                    conditional_data: vec![],
                }],
            }],
        }
    }

    #[test]
    fn reverse_subdomain_exact_match() {
        assert_eq!(reverse_subdomain("example.com"), ".moc.elpmaxe$");
    }

    #[test]
    fn reverse_subdomain_wildcard() {
        assert_eq!(reverse_subdomain("*.example.com"), ".moc.elpmaxe.");
    }

    #[test]
    fn domain_and_field_name_splits_correctly() {
        assert_eq!(
            domain_and_field_name("auth.identity.user"),
            ("auth.identity", "user")
        );
        assert_eq!(domain_and_field_name("request.path"), ("request", "path"));
        assert_eq!(domain_and_field_name("simple"), ("", "simple"));
    }

    #[test]
    fn domain_and_field_name_handles_edge_cases() {
        assert_eq!(domain_and_field_name(".field"), ("", ".field"));
        assert_eq!(domain_and_field_name("field."), ("", "field."));
        assert_eq!(domain_and_field_name("a.b.c.d"), ("a.b.c", "d"));
    }

    #[test]
    fn factory_creates_from_valid_config() {
        let config = build_test_config(vec!["example.com".to_string()], vec![], "test-service");

        let result = PipelineFactory::try_from(config);
        assert!(result.is_ok());
    }

    #[test]
    fn factory_fails_on_invalid_predicate() {
        let mut services = HashMap::new();
        services.insert(
            "test-service".to_string(),
            Service {
                service_type: ServiceType::Auth,
                endpoint: "test-cluster".to_string(),
                failure_mode: FailureMode::Deny,
                timeout: Timeout::default(),
            },
        );

        let config = PluginConfiguration {
            request_data: HashMap::new(),
            services,
            action_sets: vec![ActionSet {
                name: "test-action-set".to_string(),
                route_rule_conditions: RouteRuleConditions {
                    hostnames: vec!["example.com".to_string()],
                    predicates: vec!["invalid syntax !!!".to_string()],
                },
                actions: vec![],
            }],
        };

        let result = PipelineFactory::try_from(config);
        assert!(result.is_err());
    }

    #[test]
    fn build_returns_none_when_hostname_does_not_match() {
        let config = build_test_config(vec!["example.com".to_string()], vec![], "test-service");
        let factory = PipelineFactory::try_from(config).unwrap();

        let mock_host = MockWasmHost::new()
            .with_property("request.host".into(), "other.com".as_bytes().to_vec());
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let result = factory.build(ctx);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn build_returns_pipeline_when_hostname_matches_exact() {
        let config = build_test_config(vec!["example.com".to_string()], vec![], "test-service");
        let factory = PipelineFactory::try_from(config).unwrap();

        let mock_host = MockWasmHost::new()
            .with_property("request.host".into(), "example.com".as_bytes().to_vec());
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let result = factory.build(ctx);
        assert!(result.is_ok());
        assert!(result.unwrap().is_some());
    }

    #[test]
    fn build_returns_pipeline_when_hostname_matches_wildcard() {
        let config = build_test_config(vec!["*.example.com".to_string()], vec![], "test-service");
        let factory = PipelineFactory::try_from(config).unwrap();

        let mock_host = MockWasmHost::new()
            .with_property("request.host".into(), "api.example.com".as_bytes().to_vec());
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let result = factory.build(ctx);
        assert!(result.is_ok());
        assert!(result.unwrap().is_some());
    }

    #[test]
    fn build_returns_none_when_wildcard_does_not_match_base_domain() {
        let config = build_test_config(vec!["*.example.com".to_string()], vec![], "test-service");
        let factory = PipelineFactory::try_from(config).unwrap();

        let mock_host = MockWasmHost::new()
            .with_property("request.host".into(), "example.com".as_bytes().to_vec());
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let result = factory.build(ctx);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn build_returns_pipeline_when_route_predicates_match() {
        let config = build_test_config(
            vec!["example.com".to_string()],
            vec!["request.method == 'GET'".to_string()],
            "test-service",
        );
        let factory = PipelineFactory::try_from(config).unwrap();

        let mock_host = MockWasmHost::new()
            .with_property("request.host".into(), "example.com".as_bytes().to_vec())
            .with_property("request.method".into(), "GET".as_bytes().to_vec());
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let result = factory.build(ctx);
        assert!(result.is_ok());
        assert!(result.unwrap().is_some());
    }

    #[test]
    fn build_returns_none_when_route_predicates_do_not_match() {
        let config = build_test_config(
            vec!["example.com".to_string()],
            vec!["request.method == 'GET'".to_string()],
            "test-service",
        );
        let factory = PipelineFactory::try_from(config).unwrap();

        let mock_host = MockWasmHost::new()
            .with_property("request.host".into(), "example.com".as_bytes().to_vec())
            .with_property("request.method".into(), "POST".as_bytes().to_vec());
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let result = factory.build(ctx);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn build_returns_none_when_predicate_attribute_is_missing() {
        let config = build_test_config(
            vec!["example.com".to_string()],
            vec!["request.method == 'GET'".to_string()],
            "test-service",
        );
        let factory = PipelineFactory::try_from(config).unwrap();

        let mock_host = MockWasmHost::new()
            .with_property("request.host".into(), "example.com".as_bytes().to_vec());
        // request.method is not set, so it defaults to null in CEL
        // null == 'GET' evaluates to false (boolean), so predicate doesn't match
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let result = factory.build(ctx);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none()); // Predicate doesn't match
    }

    #[test]
    fn build_returns_error_when_route_predicate_returns_null() {
        let config = build_test_config(
            vec!["example.com".to_string()],
            vec!["request.method".to_string()], // This returns null when request.method is missing
            "test-service",
        );
        let factory = PipelineFactory::try_from(config).unwrap();

        let mock_host = MockWasmHost::new()
            .with_property("request.host".into(), "example.com".as_bytes().to_vec());
        // request.method is missing, so the expression evaluates to null (not a boolean)
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let result = factory.build(ctx);
        assert!(matches!(result, Err(BuildError::EvaluationError(_))));
    }

    #[test]
    fn build_returns_error_when_route_predicate_is_non_boolean() {
        let config = build_test_config(
            vec!["example.com".to_string()],
            vec!["request.method".to_string()], // Returns a string not a boolean
            "test-service",
        );
        let factory = PipelineFactory::try_from(config).unwrap();

        let mock_host = MockWasmHost::new()
            .with_property("request.host".into(), "example.com".as_bytes().to_vec())
            .with_property("request.method".into(), "GET".as_bytes().to_vec()); // Method IS set
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let result = factory.build(ctx);
        assert!(matches!(result, Err(BuildError::EvaluationError(_))));
    }

    #[test]
    fn build_handles_multiple_route_predicates() {
        let config = build_test_config(
            vec!["example.com".to_string()],
            vec![
                "request.method == 'POST'".to_string(),
                "request.path.startsWith('/api')".to_string(),
            ],
            "test-service",
        );
        let factory = PipelineFactory::try_from(config).unwrap();

        let mock_host = MockWasmHost::new()
            .with_property("request.host".into(), "example.com".as_bytes().to_vec())
            .with_property("request.method".into(), "POST".as_bytes().to_vec())
            .with_property("request.path".into(), "/api/users".as_bytes().to_vec());
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let result = factory.build(ctx);
        assert!(result.is_ok());
        assert!(result.unwrap().is_some());
    }

    #[test]
    fn build_fails_when_one_of_multiple_predicates_fails() {
        let config = build_test_config(
            vec!["example.com".to_string()],
            vec![
                "request.method == 'POST'".to_string(),
                "request.path.startsWith('/api')".to_string(),
            ],
            "test-service",
        );
        let factory = PipelineFactory::try_from(config).unwrap();

        let mock_host = MockWasmHost::new()
            .with_property("request.host".into(), "example.com".as_bytes().to_vec())
            .with_property("request.method".into(), "GET".as_bytes().to_vec()) // Doesn't match
            .with_property("request.path".into(), "/api/users".as_bytes().to_vec());
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let result = factory.build(ctx);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn factory_stores_request_data() {
        let mut services = HashMap::new();
        services.insert(
            "test-service".to_string(),
            Service {
                service_type: ServiceType::Auth,
                endpoint: "test-cluster".to_string(),
                failure_mode: FailureMode::Deny,
                timeout: Timeout::default(),
            },
        );

        let mut request_data = HashMap::new();
        request_data.insert(
            "metrics.labels.user".to_string(),
            "auth.identity.username".to_string(),
        );

        let config = PluginConfiguration {
            request_data,
            services,
            action_sets: vec![],
        };

        let factory = PipelineFactory::try_from(config).unwrap();
        assert_eq!(factory.request_data.len(), 1);
    }

    #[test]
    fn factory_handles_multiple_hostnames_for_same_action_set() {
        let config = build_test_config(
            vec!["example.com".to_string(), "*.api.example.com".to_string()],
            vec![],
            "test-service",
        );
        let factory = PipelineFactory::try_from(config).unwrap();

        // Test exact match
        let mock_host1 = MockWasmHost::new()
            .with_property("request.host".into(), "example.com".as_bytes().to_vec());
        let ctx1 = ReqRespCtx::new(Arc::new(mock_host1));
        assert!(factory.build(ctx1).unwrap().is_some());

        // Test wildcard match
        let mock_host2 = MockWasmHost::new().with_property(
            "request.host".into(),
            "v1.api.example.com".as_bytes().to_vec(),
        );
        let ctx2 = ReqRespCtx::new(Arc::new(mock_host2));
        assert!(factory.build(ctx2).unwrap().is_some());
    }
}
