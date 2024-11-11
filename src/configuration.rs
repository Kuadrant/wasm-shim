use std::cell::OnceCell;
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::rc::Rc;
use std::sync::Arc;

use crate::configuration::action_set::ActionSet;
use crate::configuration::action_set_index::ActionSetIndex;
use crate::data;
use crate::data::Predicate;
use crate::service::GrpcService;
use cel_interpreter::functions::time::duration;
use cel_interpreter::Value;
use serde::de::{Error, Visitor};
use serde::{Deserialize, Deserializer};
use std::time::Duration;

pub mod action;
pub mod action_set;
mod action_set_index;

#[derive(Deserialize, Debug, Clone)]
pub struct ExpressionItem {
    pub key: String,
    pub value: String,
    #[serde(skip_deserializing)]
    pub compiled: OnceCell<data::Expression>,
}

impl ExpressionItem {
    pub fn compile(&self) -> Result<(), String> {
        self.compiled
            .set(data::Expression::new(&self.value).map_err(|e| e.to_string())?)
            .expect("Expression must not be compiled yet!");
        Ok(())
    }
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

impl DataType {
    pub fn compile(&self) -> Result<(), String> {
        match self {
            DataType::Static(_) => Ok(()),
            DataType::Expression(exp) => exp.compile(),
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct DataItem {
    #[serde(flatten)]
    pub item: DataType,
}

pub struct FilterConfig {
    pub index: ActionSetIndex,
    pub services: Rc<HashMap<String, Rc<GrpcService>>>,
}

impl Default for FilterConfig {
    fn default() -> Self {
        Self {
            index: ActionSetIndex::new(),
            services: Rc::new(HashMap::new()),
        }
    }
}

impl TryFrom<PluginConfiguration> for FilterConfig {
    type Error = String;

    fn try_from(config: PluginConfiguration) -> Result<Self, Self::Error> {
        let mut index = ActionSetIndex::new();
        for action_set in config.action_sets.iter() {
            let mut predicates = Vec::default();
            for predicate in &action_set.route_rule_conditions.predicates {
                predicates.push(Predicate::route_rule(predicate).map_err(|e| e.to_string())?);
            }
            action_set
                .route_rule_conditions
                .compiled_predicates
                .set(predicates)
                .expect("Predicates must not be compiled yet!");
            for action in &action_set.actions {
                let mut predicates = Vec::default();
                for predicate in &action.predicates {
                    predicates.push(Predicate::new(predicate).map_err(|e| e.to_string())?);
                }
                action
                    .compiled_predicates
                    .set(predicates)
                    .expect("Predicates must not be compiled yet!");

                for datum in &action.data {
                    let result = datum.item.compile();
                    if result.is_err() {
                        return Err(result.err().unwrap());
                    }
                }
            }

            for hostname in action_set.route_rule_conditions.hostnames.iter() {
                index.insert(hostname, Rc::new(action_set.clone()));
            }
        }

        // configure grpc services from the services in config
        let services = config
            .services
            .into_iter()
            .map(|(name, ext)| (name, Rc::new(GrpcService::new(Rc::new(ext)))))
            .collect();

        Ok(Self {
            index,
            services: Rc::new(services),
        })
    }
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

#[derive(Deserialize, Debug, Clone, Default)]
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
impl<'de> Visitor<'de> for TimeoutVisitor {
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
            Ok(Value::Duration(duration)) => Ok(Timeout(duration.to_std().unwrap())),
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

        let filter_config = res.unwrap();
        assert_eq!(filter_config.action_sets.len(), 1);

        let services = &filter_config.services;
        assert_eq!(services.len(), 2);

        if let Some(auth_service) = services.get("authorino") {
            assert_eq!(auth_service.service_type, ServiceType::Auth);
            assert_eq!(auth_service.endpoint, "authorino-cluster");
            assert_eq!(auth_service.failure_mode, FailureMode::Deny);
            assert_eq!(auth_service.timeout, Timeout(Duration::from_millis(24)))
        } else {
            panic!()
        }

        if let Some(rl_service) = services.get("limitador") {
            assert_eq!(rl_service.service_type, ServiceType::RateLimit);
            assert_eq!(rl_service.endpoint, "limitador-cluster");
            assert_eq!(rl_service.failure_mode, FailureMode::Allow);
            assert_eq!(rl_service.timeout, Timeout(Duration::from_millis(42)))
        } else {
            panic!()
        }

        let predicates = &filter_config.action_sets[0]
            .route_rule_conditions
            .predicates;
        assert_eq!(predicates.len(), 3);

        let actions = &filter_config.action_sets[0].actions;
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

        // TODO(eastizle): DataItem does not implement PartialEq, add it only for testing?
        //assert_eq!(
        //    data_items[0],
        //    DataItem {
        //        item: DataType::Static(StaticItem {
        //            key: String::from("rlp-ns-A/rlp-name-A"),
        //            value: String::from("1")
        //        })
        //    }
        //);

        if let DataType::Static(static_item) = &rl_data_items[0].item {
            assert_eq!(static_item.key, "rlp-ns-A/rlp-name-A");
            assert_eq!(static_item.value, "1");
        } else {
            panic!();
        }

        if let DataType::Expression(exp) = &rl_data_items[1].item {
            assert_eq!(exp.key, "username");
            assert_eq!(exp.value, "auth.metadata.username");
        } else {
            panic!();
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

        let filter_config = res.unwrap();
        assert_eq!(filter_config.action_sets.len(), 0);
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

        let filter_config = res.unwrap();
        assert_eq!(filter_config.action_sets.len(), 1);

        let services = &filter_config.services;
        assert_eq!(
            services.get("limitador").unwrap().timeout,
            Timeout(Duration::from_millis(20))
        );

        let predicates = &filter_config.action_sets[0]
            .route_rule_conditions
            .predicates;
        assert_eq!(predicates.len(), 0);

        let actions = &filter_config.action_sets[0].actions;
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

    #[test]
    fn filter_config_from_configuration() {
        let res = serde_json::from_str::<PluginConfiguration>(CONFIG);
        if let Err(ref e) = res {
            eprintln!("{e}");
        }
        assert!(res.is_ok());

        let result = FilterConfig::try_from(res.unwrap());
        let filter_config = result.expect("That didn't work");
        let rlp_option = filter_config
            .index
            .get_longest_match_action_sets("example.com");
        assert!(rlp_option.is_some());

        let rlp_option = filter_config
            .index
            .get_longest_match_action_sets("test.toystore.com");
        assert!(rlp_option.is_some());

        let rlp_option = filter_config.index.get_longest_match_action_sets("unknown");
        assert!(rlp_option.is_none());
    }
}
