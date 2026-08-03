use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;

use cel::functions::time::duration;
use cel::Value;
use serde::de::{Error, Visitor};
use serde::{Deserialize, Deserializer};
use std::time::Duration;

fn default_is_guard() -> bool {
    true
}

#[derive(Deserialize, Debug, Clone, Copy, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Execution {
    #[default]
    Parallel,
    Sequential,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub predicate: String,
    pub terminal: bool,
    #[serde(default = "default_is_guard")]
    pub is_guard: bool,
    #[serde(default)]
    pub execution: Execution,
    #[serde(default)]
    pub sources: Vec<String>,
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
    pub on_reply: Vec<Action>,
    #[serde(default)]
    pub label: String,
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
    pub path: String,
    pub value: String,
    #[serde(default)]
    pub export_to_host: bool,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FailOperation {
    pub log_message: String,
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
    pub actions: Vec<Action>,
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

        let action = &actions[0];
        assert_eq!(action.predicate, "request.method == 'GET'");
        assert!(!action.terminal);

        let Operation::Grpc(grpc) = &action.operation else {
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
        let action = &plugin_config.action_sets[0].actions[0];
        assert_eq!(action.predicate, "request.path.startsWith('/admin')");
        assert!(action.terminal);
        let Operation::Deny(deny) = &action.operation else {
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
        let req_action = &plugin_config.action_sets[0].actions[0];
        let Operation::Headers(req_headers) = &req_action.operation else {
            unreachable!("expected headers operation");
        };
        assert!(matches!(req_headers.target, HeadersTarget::Request));
        assert_eq!(req_headers.headers, "auth_check.ok_response.headers");

        let resp_action = &plugin_config.action_sets[0].actions[1];
        let Operation::Headers(resp_headers) = &resp_action.operation else {
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
                    "path": "auth.metadata",
                    "value": "auth_check.dynamic_metadata"
                }]
            }]
        }"#;

        let res = serde_json::from_str::<PluginConfiguration>(config);
        assert!(res.is_ok());

        let plugin_config = res.expect("result is ok");
        let action = &plugin_config.action_sets[0].actions[0];
        let Operation::Store(store) = &action.operation else {
            unreachable!("expected store operation");
        };
        assert_eq!(store.path, "auth.metadata");
        assert_eq!(store.value, "auth_check.dynamic_metadata");
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
        let action = &plugin_config.action_sets[0].actions[0];
        let Operation::Fail(fail) = &action.operation else {
            unreachable!("expected fail operation");
        };
        assert_eq!(fail.log_message, "error has occurred");
    }

    #[test]
    fn test_is_guard_defaults_to_true() {
        let config = r#"{
            "type": "grpc",
            "predicate": "true",
            "terminal": false,
            "var": "test",
            "service": "test-service",
            "messageBuilder": "test",
            "onReply": []
        }"#;

        let action: Action = serde_json::from_str(config).expect("valid config");
        assert!(action.is_guard);
    }

    #[test]
    fn test_is_guard_can_be_set_to_false() {
        let config = r#"{
            "type": "grpc",
            "predicate": "true",
            "terminal": false,
            "isGuard": false,
            "var": "test",
            "service": "test-service",
            "messageBuilder": "test",
            "onReply": []
        }"#;

        let action: Action = serde_json::from_str(config).expect("valid config");
        assert!(!action.is_guard);
    }

    #[test]
    fn parse_action_execution_defaults_to_parallel() {
        let config = r#"{
            "type": "deny",
            "predicate": "true",
            "terminal": true,
            "denyWith": "DenyResponse{status: 403u}"
        }"#;

        let action: Action = serde_json::from_str(config).expect("valid config");
        assert_eq!(action.execution, Execution::Parallel);
    }

    #[test]
    fn parse_action_with_sequential_execution() {
        let config = r#"{
            "type": "grpc",
            "predicate": "true",
            "terminal": false,
            "execution": "sequential",
            "var": "rl",
            "service": "limitador",
            "messageBuilder": "test",
            "onReply": []
        }"#;

        let action: Action = serde_json::from_str(config).expect("valid config");
        assert_eq!(action.execution, Execution::Sequential);
    }
}
