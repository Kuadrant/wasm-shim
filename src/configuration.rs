use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;

use cel_interpreter::functions::time::duration;
use cel_interpreter::Value;
use serde::de::{Error, Visitor};
use serde::{Deserialize, Deserializer};
use std::time::Duration;

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub service: String,
    pub scope: String,
    #[serde(default)]
    pub predicates: Vec<String>,
    #[serde(default)]
    pub data: Vec<DataItem>,
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

impl ActionSet {
    #[cfg(test)]
    pub fn new(
        name: String,
        route_rule_conditions: RouteRuleConditions,
        actions: Vec<Action>,
    ) -> Self {
        ActionSet {
            name,
            route_rule_conditions,
            actions,
        }
    }
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
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PluginConfiguration {
    pub services: HashMap<String, Service>,
    pub action_sets: Vec<ActionSet>,
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

        let auth_action = &actions[0];
        assert_eq!(auth_action.service, "authorino");
        assert_eq!(auth_action.scope, "authconfig-A");

        let rl_action = &actions[1];
        assert_eq!(rl_action.service, "limitador");
        assert_eq!(rl_action.scope, "rlp-ns-A/rlp-name-A");

        let auth_data_items = &auth_action.data;
        assert_eq!(auth_data_items.len(), 0);

        let rl_data_items = &rl_action.data;
        assert_eq!(rl_data_items.len(), 2);

        let rl_predicates = &rl_action.predicates;
        assert_eq!(rl_predicates.len(), 1);

        if let DataType::Static(static_item) = &rl_data_items[0].item {
            assert_eq!(static_item.key, "rlp-ns-A/rlp-name-A");
            assert_eq!(static_item.value, "1");
        } else {
            unreachable!();
        }

        if let DataType::Expression(exp) = &rl_data_items[1].item {
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

        let action_predicates = &actions[0].predicates;
        assert_eq!(action_predicates.len(), 0);
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
                "data": [
                {
                    "unknown": {
                        "key": "rlp-ns-A/rlp-name-A",
                        "value": "1"
                    }
                }]
            }]
        }]
        }"#;
        let res = serde_json::from_str::<PluginConfiguration>(bad_config);
        assert!(res.is_err());
    }
}
