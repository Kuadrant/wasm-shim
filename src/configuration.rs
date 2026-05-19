use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;

use cel::functions::time::duration;
use cel::Value;
use serde::de::{Error, Visitor};
use serde::{Deserialize, Deserializer};
use std::time::Duration;

mod legacy_translation;
#[allow(deprecated)]
pub(crate) use legacy_translation::ratelimit::translate_legacy_ratelimit_to_typed;

#[derive(Deserialize, Debug, Clone)]
pub struct ConditionalData {
    #[serde(default)]
    pub predicates: Vec<String>,
    #[serde(default)]
    pub data: Vec<DataItem>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub service: String,
    pub scope: String,
    #[serde(default)]
    pub predicates: Vec<String>,
    #[serde(default)]
    pub conditional_data: Vec<ConditionalData>,
    #[serde(default)]
    pub sources: Vec<String>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TypedAction {
    pub predicate: String,
    pub terminal: bool,
    #[serde(flatten)]
    pub operation: Operation,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Operation {
    Grpc(GrpcOperation),
    Deny(DenyOperation),
    Headers(HeadersOperation),
    Store(StoreOperation),
    Fail(FailOperation),
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GrpcOperation {
    pub var: String,
    pub service: String,
    pub message_builder: String,
    #[serde(default)]
    pub on_reply: Vec<TypedAction>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DenyOperation {
    pub deny_with: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "lowercase")]
pub enum HeadersTarget {
    Request,
    Response,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct HeadersOperation {
    pub target: HeadersTarget,
    pub headers: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StoreOperation {
    pub data: Vec<StoreItem>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct StoreItem {
    pub path: String,
    pub value: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FailOperation {
    pub log_message: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum ActionConfig {
    Typed(TypedAction),
    Legacy(Action),
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct RouteRuleConditions {
    pub hostnames: Vec<String>,
    #[serde(default)]
    pub predicates: Vec<String>,
}

#[derive(Default, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ActionSet {
    pub name: String,
    pub route_rule_conditions: RouteRuleConditions,
    pub actions: Vec<ActionConfig>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ExpressionItem {
    pub key: String,
    pub value: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct StaticItem {
    pub value: String,
    pub key: String,
}

// Mutually exclusive struct fields
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "lowercase")]
pub enum DataType {
    Static(StaticItem),
    Expression(ExpressionItem),
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct DataItem {
    #[serde(flatten)]
    pub item: DataType,
}

#[derive(Deserialize, Debug, Copy, Clone, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum FailureMode {
    #[default]
    Deny,
    Allow,
}

#[derive(Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ServiceType {
    Auth,
    #[default]
    RateLimit,
    #[serde(rename = "ratelimit-check")]
    RateLimitCheck,
    #[serde(rename = "ratelimit-report")]
    RateLimitReport,
    Tracing,
    Dynamic,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Observability {
    pub http_header_identifier: Option<String>,
    pub default_level: Option<String>,
    pub tracing: Option<Tracing>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Tracing {
    pub service: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PluginConfiguration {
    #[serde(default)]
    pub request_data: HashMap<String, String>,
    pub services: HashMap<String, Service>,
    pub action_sets: Vec<ActionSet>,
    #[serde(default)]
    pub observability: Observability,
    #[serde(default = "default_descriptor_service")]
    pub descriptor_service: String,
}

fn default_descriptor_service() -> String {
    "kuadrant-operator-grpc".to_string()
}

impl PluginConfiguration {
    #[cfg(test)]
    pub fn new(services: HashMap<String, Service>, action_sets: Vec<ActionSet>) -> Self {
        Self {
            request_data: HashMap::new(),
            services,
            action_sets,
            observability: Default::default(),
            descriptor_service: default_descriptor_service(),
        }
    }
}

#[derive(Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    #[serde(rename = "type")]
    pub service_type: ServiceType,
    pub endpoint: String,
    // Deny/Allow request when faced with an irrecoverable failure.
    pub failure_mode: FailureMode,
    #[serde(default)]
    pub timeout: Timeout,
    pub grpc_service: Option<String>,
    pub grpc_method: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Timeout(pub Duration);
impl Default for Timeout {
    fn default() -> Self {
        Timeout(Duration::from_millis(20))
    }
}

impl<'de> Deserialize<'de> for Timeout {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(TimeoutVisitor)
    }
}

struct TimeoutVisitor;
impl Visitor<'_> for TimeoutVisitor {
    type Value = Timeout;

    fn expecting(&self, formatter: &mut Formatter) -> std::fmt::Result {
        formatter.write_str("DurationString -> Sign? Number Unit String? Sign -> '-' Number -> Digit+ ('.' Digit+)? Digit -> '0' | '1' | '2' | '3' | '4' | '5' | '6' | '7' | '8' | '9' Unit -> 'h' | 'm' | 's' | 'ms' | 'us' | 'ns' String -> DurationString")
    }

    fn visit_str<E>(self, string: &str) -> Result<Self::Value, E>
    where
        E: Error,
    {
        self.visit_string(String::from(string))
    }

    fn visit_string<E>(self, string: String) -> Result<Self::Value, E>
    where
        E: Error,
    {
        match duration(Arc::new(string)) {
            Ok(Value::Duration(duration)) => duration
                .to_std()
                .map(Timeout)
                .map_err(|e| E::custom(e.to_string())),
            Err(e) => Err(E::custom(e)),
            _ => Err(E::custom("Unsupported Duration Value")),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    const CONFIG: &str = r#"{
        "services": {
            "authorino": {
                "type": "auth",
                "endpoint": "authorino-cluster",
                "failureMode": "deny",
                "timeout": "24ms"
            },
            "limitador": {
                "type": "ratelimit",
                "endpoint": "limitador-cluster",
                "failureMode": "allow",
                "timeout": "42ms"
            }
        },
        "actionSets": [
        {
            "name": "rlp-ns-A/rlp-name-A",
            "routeRuleConditions": {
                "hostnames": ["*.toystore.com", "example.com"],
                "predicates": [
                    "request.path == '/admin/toy'",
                    "request.method == 'POST'",
                    "request.host == 'cars.toystore.com'"
                ]
            },
            "actions": [
            {
                "service": "authorino",
                "scope": "authconfig-A"
            },
            {
                "service": "limitador",
                "scope": "rlp-ns-A/rlp-name-A",
                "conditionalData": [
                {
                    "predicates": [
                        "auth.metadata.username == 'alice'"
                    ],
                    "data": [
                    {
                        "static": {
                            "key": "rlp-ns-A/rlp-name-A",
                            "value": "1"
                        }
                    },
                    {
                        "expression": {
                            "key": "username",
                            "value": "auth.metadata.username"
                        }
                    }]
                }]
            }]
        }]
    }"#;

    #[test]
    fn parse_config_happy_path() {
        let res = serde_json::from_str::<PluginConfiguration>(CONFIG);
        if let Err(ref e) = res {
            eprintln!("{e}");
        }
        assert!(res.is_ok());

        let plugin_config = res.expect("result is ok");
        assert_eq!(plugin_config.action_sets.len(), 1);

        let services = &plugin_config.services;
        assert_eq!(services.len(), 2);

        if let Some(auth_service) = services.get("authorino") {
            assert_eq!(auth_service.service_type, ServiceType::Auth);
            assert_eq!(auth_service.endpoint, "authorino-cluster");
            assert_eq!(auth_service.failure_mode, FailureMode::Deny);
            assert_eq!(auth_service.timeout, Timeout(Duration::from_millis(24)))
        } else {
            unreachable!()
        }

        if let Some(rl_service) = services.get("limitador") {
            assert_eq!(rl_service.service_type, ServiceType::RateLimit);
            assert_eq!(rl_service.endpoint, "limitador-cluster");
            assert_eq!(rl_service.failure_mode, FailureMode::Allow);
            assert_eq!(rl_service.timeout, Timeout(Duration::from_millis(42)))
        } else {
            unreachable!()
        }

        let predicates = &plugin_config.action_sets[0]
            .route_rule_conditions
            .predicates;
        assert_eq!(predicates.len(), 3);

        let actions = &plugin_config.action_sets[0].actions;
        assert_eq!(actions.len(), 2);

        let ActionConfig::Legacy(auth_action) = &actions[0] else {
            unreachable!("expected legacy action");
        };
        assert_eq!(auth_action.service, "authorino");
        assert_eq!(auth_action.scope, "authconfig-A");

        let ActionConfig::Legacy(rl_action) = &actions[1] else {
            unreachable!("expected legacy action");
        };
        assert_eq!(rl_action.service, "limitador");
        assert_eq!(rl_action.scope, "rlp-ns-A/rlp-name-A");

        let rl_conditional_data = &rl_action.conditional_data;
        assert_eq!(rl_conditional_data.len(), 1);

        let rl_conditional = &rl_conditional_data[0];
        assert_eq!(rl_conditional.predicates.len(), 1);
        assert_eq!(rl_conditional.data.len(), 2);

        let rl_predicates = &rl_action.predicates;
        assert_eq!(rl_predicates.len(), 0);

        if let DataType::Static(static_item) = &rl_conditional.data[0].item {
            assert_eq!(static_item.key, "rlp-ns-A/rlp-name-A");
            assert_eq!(static_item.value, "1");
        } else {
            unreachable!();
        }

        if let DataType::Expression(exp) = &rl_conditional.data[1].item {
            assert_eq!(exp.key, "username");
            assert_eq!(exp.value, "auth.metadata.username");
        } else {
            unreachable!();
        }
    }

    #[test]
    fn parse_config_min() {
        let config = r#"{
            "services": {},
            "actionSets": []
        }"#;
        let res = serde_json::from_str::<PluginConfiguration>(config);
        if let Err(ref e) = res {
            eprintln!("{e}");
        }
        assert!(res.is_ok());

        let plugin_config = res.expect("result is ok");
        assert_eq!(plugin_config.action_sets.len(), 0);
    }

    #[test]
    fn config_containing_data() {
        let config = r#"{
            "requestData": {
                "metrics.label1": "auth.metadata.username",
                "metrics.label2": "'id#' + request.id"
            },
            "services": {
                "limitador": {
                    "type": "ratelimit",
                    "endpoint": "limitador-cluster",
                    "failureMode": "deny"
                }
            },
            "actionSets": [
            {
                "name": "rlp-ns-A/rlp-name-A",
                "routeRuleConditions": {
                    "hostnames": ["*.toystore.com", "example.com"]
                },
                "actions": [
                {
                    "service": "limitador",
                    "scope": "rlp-ns-A/rlp-name-A",
                    "conditionalData": [
                    {
                        "data": [
                        {
                            "static": {
                                "key": "rlp-ns-A/rlp-name-A",
                                "value": "1"
                            }
                        },
                        {
                            "expression": {
                                "key": "username",
                                "value": "auth.metadata.username"
                            }
                        }]
                    }]
                }]
            }]
        }"#;

        let res = serde_json::from_str::<PluginConfiguration>(config).expect("valid config");
        assert_eq!(
            res.request_data,
            HashMap::from([
                (
                    "metrics.label1".to_owned(),
                    "auth.metadata.username".to_owned()
                ),
                ("metrics.label2".to_owned(), "'id#' + request.id".to_owned()),
            ])
        );
    }

    #[test]
    fn parse_config_predicates_optional() {
        let config = r#"{
            "services": {
                "limitador": {
                    "type": "ratelimit",
                    "endpoint": "limitador-cluster",
                    "failureMode": "deny"
                }
            },
            "actionSets": [
            {
                "name": "rlp-ns-A/rlp-name-A",
                "routeRuleConditions": {
                    "hostnames": ["*.toystore.com", "example.com"]
                },
                "actions": [
                {
                    "service": "limitador",
                    "scope": "rlp-ns-A/rlp-name-A",
                    "conditionalData": [
                    {
                        "data": [
                        {
                            "static": {
                                "key": "rlp-ns-A/rlp-name-A",
                                "value": "1"
                            }
                        },
                        {
                            "expression": {
                                "key": "username",
                                "value": "auth.metadata.username"
                            }
                        }]
                    }]
                }]
            }]
        }"#;
        let res = serde_json::from_str::<PluginConfiguration>(config);
        if let Err(ref e) = res {
            eprintln!("{e}");
        }
        assert!(res.is_ok());

        let plugin_config = res.expect("result is ok");
        assert_eq!(plugin_config.action_sets.len(), 1);

        let services = &plugin_config.services;
        assert_eq!(
            services
                .get("limitador")
                .expect("limitador service to be set")
                .timeout,
            Timeout(Duration::from_millis(20))
        );

        let predicates = &plugin_config.action_sets[0]
            .route_rule_conditions
            .predicates;
        assert_eq!(predicates.len(), 0);

        let actions = &plugin_config.action_sets[0].actions;
        assert_eq!(actions.len(), 1);

        let ActionConfig::Legacy(action) = &actions[0] else {
            unreachable!("expected legacy action");
        };
        assert_eq!(action.predicates.len(), 0);
    }

    #[test]
    fn parse_config_invalid_data() {
        // data item fields are mutually exclusive
        let bad_config = r#"{
        "services": {
            "limitador": {
                "type": "ratelimit",
                "endpoint": "limitador-cluster",
                "failureMode": "deny"
            }
        },
        "actionSets": [
        {
            "name": "rlp-ns-A/rlp-name-A",
            "routeRuleConditions": {
                "hostnames": ["*.toystore.com", "example.com"]
            },
            "actions": [
            {
                "service": "limitador",
                "scope": "rlp-ns-A/rlp-name-A",
                "conditionalData": [
                {
                    "data": [
                    {
                        "static": {
                            "key": "rlp-ns-A/rlp-name-A",
                            "value": "1"
                        },
                        "expression": {
                            "key": "username",
                            "value": "auth.metadata.username"
                        }
                    }]
                }]
            }]
        }]
        }"#;
        let res = serde_json::from_str::<PluginConfiguration>(bad_config);
        assert!(res.is_err());

        // data item unknown fields are forbidden
        let bad_config = r#"{
        "services": {
            "limitador": {
                "type": "ratelimit",
                "endpoint": "limitador-cluster",
                "failureMode": "deny"
            }
        },
        "actionSets": [
        {
            "name": "rlp-ns-A/rlp-name-A",
            "routeRuleConditions": {
                "hostnames": ["*.toystore.com", "example.com"]
            },
            "actions": [
            {
                "service": "limitador",
                "scope": "rlp-ns-A/rlp-name-A",
                "conditionalData": [
                {
                    "data": [
                    {
                        "unknown": {
                            "key": "rlp-ns-A/rlp-name-A",
                            "value": "1"
                        }
                    }]
                }]
            }]
        }]
        }"#;
        let res = serde_json::from_str::<PluginConfiguration>(bad_config);
        assert!(res.is_err());
    }

    #[test]
    fn parse_dynamic_service_config() {
        let config = r#"{
            "services": {
                "limitador-dynamic": {
                    "type": "dynamic",
                    "endpoint": "limitador-cluster",
                    "failureMode": "deny",
                    "timeout": "1s",
                    "grpcService": "envoy.service.ratelimit.v3.RateLimitService",
                    "grpcMethod": "ShouldRateLimit"
                }
            },
            "actionSets": []
        }"#;

        let res = serde_json::from_str::<PluginConfiguration>(config);
        if let Err(ref e) = res {
            eprintln!("{e}");
        }
        assert!(res.is_ok());

        let plugin_config = res.expect("result is ok");
        let dynamic_service = plugin_config
            .services
            .get("limitador-dynamic")
            .expect("dynamic service to be set");

        assert_eq!(dynamic_service.service_type, ServiceType::Dynamic);
        assert_eq!(dynamic_service.endpoint, "limitador-cluster");
        assert_eq!(dynamic_service.failure_mode, FailureMode::Deny);
        assert_eq!(
            dynamic_service.grpc_service.as_ref(),
            Some(&"envoy.service.ratelimit.v3.RateLimitService".to_string())
        );
        assert_eq!(
            dynamic_service.grpc_method.as_ref(),
            Some(&"ShouldRateLimit".to_string())
        );
    }

    #[test]
    fn parse_grpc_action_with_on_reply() {
        let config = r#"{
            "services": {
                "limitador": {
                    "type": "dynamic",
                    "endpoint": "limitador-cluster",
                    "failureMode": "deny",
                    "timeout": "100ms",
                    "grpcService": "envoy.service.ratelimit.v3.RateLimitService",
                    "grpcMethod": "ShouldRateLimit"
                }
            },
            "actionSets": [{
                "name": "test-rl",
                "routeRuleConditions": {
                    "hostnames": ["api.example.com"]
                },
                "actions": [{
                    "type": "grpc",
                    "predicate": "request.method == 'GET'",
                    "terminal": false,
                    "var": "rl_check",
                    "service": "limitador",
                    "messageBuilder": "envoy.service.ratelimit.v3.RateLimitRequest { domain: 'test' }",
                    "onReply": [
                        {
                            "type": "deny",
                            "predicate": "rl_check.overall_code == 2",
                            "terminal": true,
                            "denyWith": "DenyResponse{status: 429u}"
                        },
                        {
                            "type": "headers",
                            "predicate": "true",
                            "terminal": false,
                            "target": "response",
                            "headers": "rl_check.response_headers_to_add"
                        }
                    ]
                }]
            }]
        }"#;

        let res = serde_json::from_str::<PluginConfiguration>(config);
        if let Err(ref e) = res {
            eprintln!("{e}");
        }
        assert!(res.is_ok());

        let plugin_config = res.expect("result is ok");
        let actions = &plugin_config.action_sets[0].actions;
        assert_eq!(actions.len(), 1);

        let ActionConfig::Typed(typed) = &actions[0] else {
            unreachable!("expected typed action");
        };
        assert_eq!(typed.predicate, "request.method == 'GET'");
        assert!(!typed.terminal);

        let Operation::Grpc(grpc) = &typed.operation else {
            unreachable!("expected grpc operation");
        };
        assert_eq!(grpc.var, "rl_check");
        assert_eq!(grpc.service, "limitador");
        assert_eq!(
            grpc.message_builder,
            "envoy.service.ratelimit.v3.RateLimitRequest { domain: 'test' }"
        );
        assert_eq!(grpc.on_reply.len(), 2);

        let reply_deny = &grpc.on_reply[0];
        assert_eq!(reply_deny.predicate, "rl_check.overall_code == 2");
        assert!(reply_deny.terminal);
        let Operation::Deny(deny) = &reply_deny.operation else {
            unreachable!("expected deny operation");
        };
        assert_eq!(deny.deny_with, "DenyResponse{status: 429u}");

        let reply_headers = &grpc.on_reply[1];
        assert!(!reply_headers.terminal);
        let Operation::Headers(headers) = &reply_headers.operation else {
            unreachable!("expected headers operation");
        };
        assert!(matches!(headers.target, HeadersTarget::Response));
        assert_eq!(headers.headers, "rl_check.response_headers_to_add");
    }

    #[test]
    fn parse_deny_action() {
        let config = r#"{
            "services": {},
            "actionSets": [{
                "name": "test-deny",
                "routeRuleConditions": {
                    "hostnames": ["example.com"]
                },
                "actions": [{
                    "type": "deny",
                    "predicate": "request.path.startsWith('/admin')",
                    "terminal": true,
                    "denyWith": "DenyResponse{status: 403u}"
                }]
            }]
        }"#;

        let res = serde_json::from_str::<PluginConfiguration>(config);
        assert!(res.is_ok());

        let plugin_config = res.expect("result is ok");
        let ActionConfig::Typed(typed) = &plugin_config.action_sets[0].actions[0] else {
            unreachable!("expected typed action");
        };
        assert_eq!(typed.predicate, "request.path.startsWith('/admin')");
        assert!(typed.terminal);
        let Operation::Deny(deny) = &typed.operation else {
            unreachable!("expected deny operation");
        };
        assert_eq!(deny.deny_with, "DenyResponse{status: 403u}");
    }

    #[test]
    fn parse_headers_action() {
        let config = r#"{
            "services": {},
            "actionSets": [{
                "name": "test-headers",
                "routeRuleConditions": {
                    "hostnames": ["example.com"]
                },
                "actions": [
                    {
                        "type": "headers",
                        "predicate": "has(auth_check.ok_response)",
                        "terminal": false,
                        "target": "request",
                        "headers": "auth_check.ok_response.headers"
                    },
                    {
                        "type": "headers",
                        "predicate": "true",
                        "terminal": false,
                        "target": "response",
                        "headers": "rl_check.response_headers_to_add"
                    }
                ]
            }]
        }"#;

        let res = serde_json::from_str::<PluginConfiguration>(config);
        assert!(res.is_ok());

        let plugin_config = res.expect("result is ok");
        let ActionConfig::Typed(typed_req) = &plugin_config.action_sets[0].actions[0] else {
            unreachable!("expected typed action");
        };
        let Operation::Headers(req_headers) = &typed_req.operation else {
            unreachable!("expected headers operation");
        };
        assert!(matches!(req_headers.target, HeadersTarget::Request));
        assert_eq!(req_headers.headers, "auth_check.ok_response.headers");

        let ActionConfig::Typed(typed_resp) = &plugin_config.action_sets[0].actions[1] else {
            unreachable!("expected typed action");
        };
        let Operation::Headers(resp_headers) = &typed_resp.operation else {
            unreachable!("expected headers operation");
        };
        assert!(matches!(resp_headers.target, HeadersTarget::Response));
        assert_eq!(resp_headers.headers, "rl_check.response_headers_to_add");
    }

    #[test]
    fn parse_store_action() {
        let config = r#"{
            "services": {},
            "actionSets": [{
                "name": "test-store",
                "routeRuleConditions": {
                    "hostnames": ["example.com"]
                },
                "actions": [{
                    "type": "store",
                    "predicate": "true",
                    "terminal": false,
                    "data": [
                        { "path": "auth.metadata", "value": "auth_check.dynamic_metadata" },
                        { "path": "auth.identity", "value": "auth_check.identity" }
                    ]
                }]
            }]
        }"#;

        let res = serde_json::from_str::<PluginConfiguration>(config);
        assert!(res.is_ok());

        let plugin_config = res.expect("result is ok");
        let ActionConfig::Typed(typed) = &plugin_config.action_sets[0].actions[0] else {
            unreachable!("expected typed action");
        };
        let Operation::Store(store) = &typed.operation else {
            unreachable!("expected store operation");
        };
        assert_eq!(store.data.len(), 2);
        assert_eq!(store.data[0].path, "auth.metadata");
        assert_eq!(store.data[0].value, "auth_check.dynamic_metadata");
        assert_eq!(store.data[1].path, "auth.identity");
        assert_eq!(store.data[1].value, "auth_check.identity");
    }

    #[test]
    fn parse_fail_action() {
        let config = r#"{
            "services": {},
            "actionSets": [{
                "name": "test-store",
                "routeRuleConditions": {
                    "hostnames": ["example.com"]
                },
                "actions": [{
                    "type": "fail",
                    "predicate": "true",
                    "terminal": true,
                    "logMessage": "error has occurred"
                }]
            }]
        }"#;

        let res = serde_json::from_str::<PluginConfiguration>(config);
        assert!(res.is_ok());

        let plugin_config = res.expect("result is ok");
        let ActionConfig::Typed(typed) = &plugin_config.action_sets[0].actions[0] else {
            unreachable!("expected typed action");
        };
        let Operation::Fail(fail) = &typed.operation else {
            unreachable!("expected fail operation");
        };
        assert_eq!(fail.log_message, "error has occurred");
    }

    #[test]
    fn parse_mixed_legacy_and_typed_actions() {
        let config = r#"{
            "services": {
                "authorino": {
                    "type": "auth",
                    "endpoint": "authorino-cluster",
                    "failureMode": "deny",
                    "timeout": "24ms"
                },
                "limitador": {
                    "type": "dynamic",
                    "endpoint": "limitador-cluster",
                    "failureMode": "deny",
                    "timeout": "100ms",
                    "grpcService": "envoy.service.ratelimit.v3.RateLimitService",
                    "grpcMethod": "ShouldRateLimit"
                }
            },
            "actionSets": [{
                "name": "mixed",
                "routeRuleConditions": {
                    "hostnames": ["example.com"]
                },
                "actions": [
                    {
                        "service": "authorino",
                        "scope": "authconfig-A"
                    },
                    {
                        "type": "grpc",
                        "predicate": "true",
                        "terminal": false,
                        "var": "rl_check",
                        "service": "limitador",
                        "messageBuilder": "envoy.service.ratelimit.v3.RateLimitRequest { domain: 'test' }",
                        "onReply": [{
                            "type": "deny",
                            "predicate": "rl_check.overall_code == 2",
                            "terminal": true,
                            "denyWith": "DenyResponse{status: 429u}"
                        }]
                    }
                ]
            }]
        }"#;

        let res = serde_json::from_str::<PluginConfiguration>(config);
        if let Err(ref e) = res {
            eprintln!("{e}");
        }
        assert!(res.is_ok());

        let plugin_config = res.expect("result is ok");
        let actions = &plugin_config.action_sets[0].actions;
        assert_eq!(actions.len(), 2);

        assert!(matches!(&actions[0], ActionConfig::Legacy(_)));
        assert!(matches!(
            &actions[1],
            ActionConfig::Typed(TypedAction {
                operation: Operation::Grpc(_),
                ..
            })
        ));
    }
}
