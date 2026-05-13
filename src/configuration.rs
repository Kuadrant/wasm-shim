use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;

use cel::functions::time::duration;
use cel::Value;
use serde::de::{Error, Visitor};
use serde::{Deserialize, Deserializer};
use std::time::Duration;

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

fn escape_cel_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

const RATELIMIT_KNOWN_ATTRS: [&str; 2] = ["ratelimit.domain", "ratelimit.hits_addend"];

fn is_ratelimit_known_attr(item: &DataItem) -> bool {
    let key = match &item.item {
        DataType::Static(s) => s.key.as_str(),
        DataType::Expression(e) => e.key.as_str(),
    };
    RATELIMIT_KNOWN_ATTRS.contains(&key)
}

fn find_ratelimit_known_attr_cel(
    conditional_data: &[ConditionalData],
    attr_key: &str,
) -> Option<String> {
    for cd in conditional_data {
        for item in &cd.data {
            match &item.item {
                DataType::Static(s) if s.key == attr_key => {
                    return Some(format!(r#""{}""#, escape_cel_string(&s.value)));
                }
                DataType::Expression(e) if e.key == attr_key => {
                    return Some(e.value.clone());
                }
                _ => {}
            }
        }
    }
    None
}

fn build_ratelimit_descriptor_entry_cel(item: &DataItem) -> String {
    let (key, value_cel) = match &item.item {
        DataType::Static(s) => (
            s.key.as_str(),
            format!(r#""{}""#, escape_cel_string(&s.value)),
        ),
        DataType::Expression(e) => (e.key.as_str(), format!("string({})", e.value)),
    };

    format!(
        r#"envoy.extensions.common.ratelimit.v3.RateLimitDescriptor.Entry {{ key: "{}", value: {} }}"#,
        escape_cel_string(key),
        value_cel,
    )
}

fn build_ratelimit_entry_list_cel(cd: &ConditionalData) -> Option<String> {
    let entries: Vec<String> = cd
        .data
        .iter()
        .filter(|item| !is_ratelimit_known_attr(item))
        .map(build_ratelimit_descriptor_entry_cel)
        .collect();

    if entries.is_empty() {
        return None;
    }

    let entries_list = format!("[{}]", entries.join(", "));

    if cd.predicates.is_empty() {
        Some(entries_list)
    } else {
        let predicate_cel = cd.predicates.join(" && ");
        Some(format!("(({}) ? {} : [])", predicate_cel, entries_list))
    }
}

fn build_ratelimit_descriptors_cel(conditional_data: &[ConditionalData]) -> String {
    let entry_parts: Vec<String> = conditional_data
        .iter()
        .filter_map(build_ratelimit_entry_list_cel)
        .collect();

    if entry_parts.is_empty() {
        return "[]".to_string();
    }

    let combined_entries = entry_parts.join(" + ");
    format!(
        "[envoy.extensions.common.ratelimit.v3.RateLimitDescriptor {{ entries: {} }}]",
        combined_entries
    )
}

fn build_ratelimit_message_builder(
    scope: &str,
    conditional_data: &[ConditionalData],
    request_data: &[((String, String), String)],
) -> String {
    let domain_cel = find_ratelimit_known_attr_cel(conditional_data, "ratelimit.domain")
        .unwrap_or_else(|| format!(r#""{}""#, escape_cel_string(scope)));

    let hits_addend_cel = find_ratelimit_known_attr_cel(conditional_data, "ratelimit.hits_addend")
        .unwrap_or_else(|| "1u".to_string());

    let mut descriptors = vec![];

    let cond_descriptors_cel = build_ratelimit_descriptors_cel(conditional_data);
    if cond_descriptors_cel != "[]" {
        descriptors.push(
            cond_descriptors_cel
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_string(),
        );
    }

    if !request_data.is_empty() {
        let request_data_entries: Vec<String> = request_data
            .iter()
            .map(|((domain, field), value_expr)| {
                let key = if domain.is_empty() || domain == "metrics.labels" {
                    field.clone()
                } else {
                    format!("{}.{}", domain, field)
                };
                format!(
                    r#"envoy.extensions.common.ratelimit.v3.RateLimitDescriptor.Entry {{ key: "{}", value: {} }}"#,
                    escape_cel_string(&key),
                    value_expr
                )
            })
            .collect();

        if !request_data_entries.is_empty() {
            descriptors.push(format!(
                "envoy.extensions.common.ratelimit.v3.RateLimitDescriptor {{ entries: [{}] }}",
                request_data_entries.join(", ")
            ));
        }
    }

    let descriptors_cel = if descriptors.is_empty() {
        "[]".to_string()
    } else {
        format!("[{}]", descriptors.join(", "))
    };

    format!(
        r#"envoy.service.ratelimit.v3.RateLimitRequest {{
    domain: {},
    hits_addend: {},
    descriptors: {}
}}"#,
        domain_cel, hits_addend_cel, descriptors_cel,
    )
}

fn build_descriptor_predicate(conditional_data: &[ConditionalData]) -> String {
    if conditional_data.is_empty() {
        return "true".to_string();
    }
    if conditional_data.iter().any(|cd| cd.predicates.is_empty()) {
        return "true".to_string();
    }

    let block_predicates: Vec<String> = conditional_data
        .iter()
        .map(|cd| {
            if cd.predicates.len() == 1 {
                cd.predicates[0].clone()
            } else {
                let wrapped: Vec<String> =
                    cd.predicates.iter().map(|p| format!("({})", p)).collect();
                format!("({})", wrapped.join(" && "))
            }
        })
        .collect();

    if block_predicates.len() == 1 {
        block_predicates[0].clone()
    } else {
        block_predicates.join(" || ")
    }
}

fn build_action_predicate(action_predicates: &[String]) -> String {
    if action_predicates.is_empty() {
        "true".to_string()
    } else if action_predicates.len() == 1 {
        action_predicates[0].clone()
    } else {
        let wrapped: Vec<String> = action_predicates
            .iter()
            .map(|p| format!("({})", p))
            .collect();
        wrapped.join(" && ")
    }
}

fn build_ratelimit_predicate(
    action_predicates: &[String],
    conditional_data: &[ConditionalData],
) -> String {
    let action_pred = build_action_predicate(action_predicates);
    let conditional_pred = build_descriptor_predicate(conditional_data);

    if action_pred == "true" && conditional_pred == "true" {
        "true".to_string()
    } else if action_pred == "true" {
        conditional_pred
    } else if conditional_pred == "true" {
        action_pred
    } else {
        format!("({}) && ({})", action_pred, conditional_pred)
    }
}

fn build_ratelimit_on_reply(name: &str) -> Vec<TypedAction> {
    vec![
        TypedAction {
            predicate: format!("{}.overall_code == 2", name),
            terminal: true,
            operation: Operation::Deny(DenyOperation {
                deny_with: format!(
                    r#"DenyResponse{{status: 429u, headers: {}.response_headers_to_add, body: "Too Many Requests\n"}}"#,
                    name
                ),
            }),
        },
        TypedAction {
            predicate: format!("{}.overall_code == 1", name),
            terminal: false,
            operation: Operation::Headers(HeadersOperation {
                target: HeadersTarget::Response,
                headers: format!("{}.response_headers_to_add", name),
            }),
        },
        TypedAction {
            predicate: format!("{}.overall_code != 1 && {}.overall_code != 2", name, name),
            terminal: true,
            operation: Operation::Fail(FailOperation {
                log_message: format!("Unknown rate limit response code from {}", name),
            }),
        },
    ]
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

    #[test]
    fn test_build_ratelimit_message_builder_simple() {
        let scope = "my-ratelimit";
        let conditional_data = vec![];
        let request_data = vec![];

        let result = build_ratelimit_message_builder(scope, &conditional_data, &request_data);

        assert!(result.contains(r#"domain: "my-ratelimit""#));
        assert!(result.contains("hits_addend: 1u"));
        assert!(result.contains("descriptors: []"));
    }

    #[test]
    fn test_build_ratelimit_message_builder_with_custom_domain() {
        let scope = "my-ratelimit";
        let conditional_data = vec![ConditionalData {
            predicates: vec![],
            data: vec![DataItem {
                item: DataType::Static(StaticItem {
                    key: "ratelimit.domain".to_string(),
                    value: "custom-domain".to_string(),
                }),
            }],
        }];
        let request_data = vec![];

        let result = build_ratelimit_message_builder(scope, &conditional_data, &request_data);

        assert!(result.contains(r#"domain: "custom-domain""#));
    }

    #[test]
    fn test_build_ratelimit_message_builder_with_hits_addend() {
        let scope = "my-ratelimit";
        let conditional_data = vec![ConditionalData {
            predicates: vec![],
            data: vec![DataItem {
                item: DataType::Expression(ExpressionItem {
                    key: "ratelimit.hits_addend".to_string(),
                    value: "5u".to_string(),
                }),
            }],
        }];
        let request_data = vec![];

        let result = build_ratelimit_message_builder(scope, &conditional_data, &request_data);

        assert!(result.contains("hits_addend: 5u"));
    }

    #[test]
    fn test_build_ratelimit_message_builder_with_conditional_data() {
        let scope = "my-ratelimit";
        let conditional_data = vec![ConditionalData {
            predicates: vec!["auth.identity.user == 'alice'".to_string()],
            data: vec![
                DataItem {
                    item: DataType::Static(StaticItem {
                        key: "limit".to_string(),
                        value: "10".to_string(),
                    }),
                },
                DataItem {
                    item: DataType::Expression(ExpressionItem {
                        key: "username".to_string(),
                        value: "auth.identity.user".to_string(),
                    }),
                },
            ],
        }];
        let request_data = vec![];

        let result = build_ratelimit_message_builder(scope, &conditional_data, &request_data);

        assert!(result.contains("auth.identity.user == 'alice'"));
        assert!(result.contains(r#"key: "limit""#));
        assert!(result.contains(r#"value: "10""#));
        assert!(result.contains(r#"key: "username""#));
        assert!(result.contains("value: string(auth.identity.user)"));
    }

    #[test]
    fn test_build_ratelimit_message_builder_with_request_data() {
        let scope = "my-ratelimit";
        let conditional_data = vec![];
        let request_data = vec![
            (
                ("metrics.labels".to_string(), "user".to_string()),
                "auth.identity.user".to_string(),
            ),
            (
                ("metrics.labels".to_string(), "env".to_string()),
                r#""production""#.to_string(),
            ),
        ];

        let result = build_ratelimit_message_builder(scope, &conditional_data, &request_data);

        assert!(result.contains(r#"key: "user""#));
        assert!(result.contains("value: auth.identity.user"));
        assert!(result.contains(r#"key: "env""#));
        assert!(result.contains(r#"value: "production""#));
    }

    #[test]
    fn test_build_ratelimit_message_builder_mixed() {
        let scope = "my-ratelimit";
        let conditional_data = vec![
            ConditionalData {
                predicates: vec!["auth.identity.role == 'admin'".to_string()],
                data: vec![DataItem {
                    item: DataType::Static(StaticItem {
                        key: "tier".to_string(),
                        value: "premium".to_string(),
                    }),
                }],
            },
            ConditionalData {
                predicates: vec![],
                data: vec![DataItem {
                    item: DataType::Expression(ExpressionItem {
                        key: "method".to_string(),
                        value: "request.method".to_string(),
                    }),
                }],
            },
        ];
        let request_data = vec![(
            ("".to_string(), "zone".to_string()),
            r#""us-east""#.to_string(),
        )];

        let result = build_ratelimit_message_builder(scope, &conditional_data, &request_data);

        assert!(result.contains("descriptors:"));
        assert!(result.contains("auth.identity.role == 'admin'"));
        assert!(result.contains(r#"key: "tier""#));
        assert!(result.contains(r#"key: "method""#));
        assert!(result.contains(r#"key: "zone""#));
    }

    #[test]
    fn test_escape_cel_string() {
        assert_eq!(escape_cel_string("simple"), "simple");
        assert_eq!(escape_cel_string("with\"quotes"), r#"with\"quotes"#);
        assert_eq!(escape_cel_string("with\\backslash"), r"with\\backslash");
        assert_eq!(escape_cel_string("with\nnewline"), r"with\nnewline");
        assert_eq!(escape_cel_string("with\ttab"), r"with\ttab");
    }

    #[test]
    fn test_build_descriptor_predicate_empty() {
        let conditional_data = vec![];
        assert_eq!(build_descriptor_predicate(&conditional_data), "true");
    }

    #[test]
    fn test_build_descriptor_predicate_unconditional() {
        let conditional_data = vec![ConditionalData {
            predicates: vec![],
            data: vec![DataItem {
                item: DataType::Static(StaticItem {
                    key: "limit".to_string(),
                    value: "10".to_string(),
                }),
            }],
        }];
        assert_eq!(build_descriptor_predicate(&conditional_data), "true");
    }

    #[test]
    fn test_build_descriptor_predicate_single_predicate() {
        let conditional_data = vec![ConditionalData {
            predicates: vec!["auth.identity.user == 'alice'".to_string()],
            data: vec![],
        }];
        assert_eq!(
            build_descriptor_predicate(&conditional_data),
            "auth.identity.user == 'alice'"
        );
    }

    #[test]
    fn test_build_descriptor_predicate_multiple_predicates_single_block() {
        let conditional_data = vec![ConditionalData {
            predicates: vec![
                "auth.identity.user == 'alice'".to_string(),
                "request.method == 'POST'".to_string(),
            ],
            data: vec![],
        }];
        assert_eq!(
            build_descriptor_predicate(&conditional_data),
            "((auth.identity.user == 'alice') && (request.method == 'POST'))"
        );
    }

    #[test]
    fn test_build_descriptor_predicate_multiple_blocks() {
        let conditional_data = vec![
            ConditionalData {
                predicates: vec!["auth.identity.role == 'admin'".to_string()],
                data: vec![],
            },
            ConditionalData {
                predicates: vec!["auth.identity.role == 'user'".to_string()],
                data: vec![],
            },
        ];
        assert_eq!(
            build_descriptor_predicate(&conditional_data),
            "auth.identity.role == 'admin' || auth.identity.role == 'user'"
        );
    }

    #[test]
    fn test_build_descriptor_predicate_mixed_conditional_unconditional() {
        let conditional_data = vec![
            ConditionalData {
                predicates: vec!["auth.identity.role == 'admin'".to_string()],
                data: vec![],
            },
            ConditionalData {
                predicates: vec![],
                data: vec![],
            },
        ];
        assert_eq!(build_descriptor_predicate(&conditional_data), "true");
    }

    #[test]
    fn test_build_ratelimit_on_reply_structure() {
        let on_reply = build_ratelimit_on_reply("rl_response");

        assert_eq!(on_reply.len(), 3);

        assert_eq!(on_reply[0].predicate, "rl_response.overall_code == 2");
        assert!(on_reply[0].terminal);
        assert!(matches!(on_reply[0].operation, Operation::Deny(_)));

        assert_eq!(on_reply[1].predicate, "rl_response.overall_code == 1");
        assert!(!on_reply[1].terminal);
        assert!(matches!(on_reply[1].operation, Operation::Headers(_)));

        assert_eq!(
            on_reply[2].predicate,
            "rl_response.overall_code != 1 && rl_response.overall_code != 2"
        );
        assert!(on_reply[2].terminal);
        assert!(matches!(on_reply[2].operation, Operation::Fail(_)));
    }

    #[test]
    fn test_build_ratelimit_on_reply_deny_operation() {
        let on_reply = build_ratelimit_on_reply("test_var");

        assert!(matches!(&on_reply[0].operation,
            Operation::Deny(deny_op) if
                deny_op.deny_with.contains("DenyResponse") &&
                deny_op.deny_with.contains("status: 429u") &&
                deny_op.deny_with.contains("test_var.response_headers_to_add") &&
                deny_op.deny_with.contains("Too Many Requests")
        ));
    }

    #[test]
    fn test_build_ratelimit_on_reply_headers_operation() {
        let on_reply = build_ratelimit_on_reply("my_rl");

        assert!(matches!(&on_reply[1].operation,
            Operation::Headers(headers_op) if
                matches!(headers_op.target, HeadersTarget::Response) &&
                headers_op.headers == "my_rl.response_headers_to_add"
        ));
    }

    #[test]
    fn test_build_ratelimit_on_reply_fail_operation() {
        let on_reply = build_ratelimit_on_reply("rate_limit");

        assert!(matches!(&on_reply[2].operation,
            Operation::Fail(fail_op) if
                fail_op.log_message.contains("Unknown rate limit response code") &&
                fail_op.log_message.contains("rate_limit")
        ));
    }
}
