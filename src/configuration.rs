use std::cell::OnceCell;
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::rc::Rc;
use std::sync::Arc;

use crate::configuration::action_set::ActionSet;
use crate::configuration::action_set_index::ActionSetIndex;
use crate::data;
use crate::data::PropertyPath;
use crate::data::{AttributeValue, Predicate};
use crate::service::GrpcService;
use cel_interpreter::functions::duration;
use cel_interpreter::objects::ValueType;
use cel_interpreter::{Context, Expression, Value};
use cel_parser::{Atom, RelationOp};
use log::debug;
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
pub struct SelectorItem {
    // Selector of an attribute from the contextual properties provided by kuadrant
    // during request and connection processing
    pub selector: String,

    // If not set it defaults to `selector` field value as the descriptor key.
    #[serde(default)]
    pub key: Option<String>,

    // An optional value to use if the selector is not found in the context.
    // If not set and the selector is not found in the context, then no data is generated.
    #[serde(default)]
    pub default: Option<String>,

    #[serde(skip_deserializing)]
    path: OnceCell<PropertyPath>,
}

impl SelectorItem {
    pub fn compile(&self) -> Result<(), String> {
        self.path
            .set(self.selector.as_str().into())
            .map_err(|p| format!("Err on {p:?}"))
    }

    pub fn path(&self) -> &PropertyPath {
        self.path
            .get()
            .expect("SelectorItem wasn't previously compiled!")
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
    Selector(SelectorItem),
    Expression(ExpressionItem),
}

impl DataType {
    pub fn compile(&self) -> Result<(), String> {
        match self {
            DataType::Static(_) => Ok(()),
            DataType::Selector(selector) => selector.compile(),
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

#[derive(Deserialize, PartialEq, Debug, Clone)]
pub enum WhenConditionOperator {
    #[serde(rename = "eq")]
    Equal,
    #[serde(rename = "neq")]
    NotEqual,
    #[serde(rename = "startswith")]
    StartsWith,
    #[serde(rename = "endswith")]
    EndsWith,
    #[serde(rename = "matches")]
    Matches,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PatternExpression {
    pub selector: String,
    pub operator: WhenConditionOperator,
    pub value: String,

    #[serde(skip_deserializing)]
    path: OnceCell<PropertyPath>,
    #[serde(skip_deserializing)]
    compiled: OnceCell<CelExpression>,
}

impl PatternExpression {
    pub fn compile(&self) -> Result<(), String> {
        self.path
            .set(self.selector.as_str().into())
            .map_err(|_| "Duh!")?;
        self.compiled
            .set(self.try_into()?)
            .map_err(|_| "Ooops".to_string())
    }
    pub fn path(&self) -> &PropertyPath {
        self.path
            .get()
            .expect("PatternExpression wasn't previously compiled!")
    }

    pub fn eval(&self, raw_attribute: Vec<u8>) -> Result<bool, String> {
        let cel_type = &self.compiled.get().unwrap().cel_type;
        let value = match cel_type {
            ValueType::String => Value::String(Arc::new(AttributeValue::parse(raw_attribute)?)),
            ValueType::Int => Value::Int(AttributeValue::parse(raw_attribute)?),
            ValueType::UInt => Value::UInt(AttributeValue::parse(raw_attribute)?),
            ValueType::Float => Value::Float(AttributeValue::parse(raw_attribute)?),
            ValueType::Bytes => Value::Bytes(Arc::new(AttributeValue::parse(raw_attribute)?)),
            ValueType::Bool => Value::Bool(AttributeValue::parse(raw_attribute)?),
            ValueType::Timestamp => Value::Timestamp(AttributeValue::parse(raw_attribute)?),
            // todo: Impl support for parsing these two types… Tho List/Map of what?
            // ValueType::List => {}
            // ValueType::Map => {}
            _ => unimplemented!("Need support for {}", cel_type),
        };
        let mut ctx = Context::default();
        ctx.add_variable_from_value("attribute", value);
        Value::resolve(&self.compiled.get().unwrap().expression, &ctx)
            .map(|v| {
                if let Value::Bool(result) = v {
                    result
                } else {
                    false
                }
            })
            .map_err(|err| format!("Error evaluating {:?}: {}", self.compiled, err))
    }

    fn applies(&self) -> bool {
        let attribute_value = match crate::data::get_property(self.path()).unwrap() {
            //TODO(didierofrivia): Replace hostcalls by DI
            None => {
                debug!(
                    "pattern_expression_applies:  selector not found: {}, defaulting to ``",
                    self.selector
                );
                b"".to_vec()
            }
            Some(attribute_bytes) => attribute_bytes,
        };

        // if someone would have the P_E be:
        // selector: auth.identity.anonymous
        // operator: eq
        // value: \""true"\"
        self.eval(attribute_value).unwrap_or_else(|e| {
            debug!("pattern_expression_applies failed: {}", e);
            false
        })
    }
}

struct CelExpression {
    expression: Expression,
    cel_type: ValueType,
}

impl Debug for CelExpression {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "CelExpression({}, {:?}", self.cel_type, self.expression)
    }
}

impl Clone for CelExpression {
    fn clone(&self) -> Self {
        Self {
            expression: self.expression.clone(),
            cel_type: match self.cel_type {
                ValueType::List => ValueType::List,
                ValueType::Map => ValueType::Map,
                ValueType::Function => ValueType::Function,
                ValueType::Int => ValueType::Int,
                ValueType::UInt => ValueType::UInt,
                ValueType::Float => ValueType::Float,
                ValueType::String => ValueType::String,
                ValueType::Bytes => ValueType::Bytes,
                ValueType::Bool => ValueType::Bool,
                ValueType::Duration => ValueType::Duration,
                ValueType::Timestamp => ValueType::Timestamp,
                ValueType::Null => ValueType::Null,
            },
        }
    }
}

impl TryFrom<&PatternExpression> for CelExpression {
    type Error = String;

    fn try_from(expression: &PatternExpression) -> Result<Self, Self::Error> {
        let cel_value = match cel_parser::parse(&expression.value) {
            Ok(exp) => match exp {
                Expression::Ident(ident) => Expression::Atom(Atom::String(ident)),
                Expression::Member(_, _) => {
                    Expression::Atom(Atom::String(expression.value.to_string().into()))
                }
                _ => exp,
            },
            Err(_) => Expression::Atom(Atom::String(expression.value.clone().into())),
        };
        let cel_type = type_of(&expression.selector).unwrap_or(match &cel_value {
            Expression::List(_) => Ok(ValueType::List),
            Expression::Map(_) => Ok(ValueType::Map),
            Expression::Atom(atom) => Ok(match atom {
                Atom::Int(_) => ValueType::Int,
                Atom::UInt(_) => ValueType::UInt,
                Atom::Float(_) => ValueType::Float,
                Atom::String(_) => ValueType::String,
                Atom::Bytes(_) => ValueType::Bytes,
                Atom::Bool(_) => ValueType::Bool,
                Atom::Null => ValueType::Null,
            }),
            _ => Err(format!("Unsupported CEL value: {cel_value:?}")),
        }?);

        let value = match cel_type {
            ValueType::Map => match expression.operator {
                WhenConditionOperator::Equal | WhenConditionOperator::NotEqual => {
                    if let Expression::Map(data) = cel_value {
                        Ok(Expression::Map(data))
                    } else {
                        Err(format!("Can't compare {cel_value:?} with a Map"))
                    }
                }
                _ => Err(format!(
                    "Unsupported operator {:?} on Map",
                    &expression.operator
                )),
            },
            ValueType::Int | ValueType::UInt | ValueType::Float => match expression.operator {
                WhenConditionOperator::Equal | WhenConditionOperator::NotEqual => {
                    if let Expression::Atom(atom) = &cel_value {
                        match atom {
                            Atom::Int(_) | Atom::UInt(_) | Atom::Float(_) => Ok(cel_value),
                            _ => Err(format!("Can't compare {cel_value:?} with a Number")),
                        }
                    } else {
                        Err(format!("Can't compare {cel_value:?} with a Number"))
                    }
                }
                _ => Err(format!(
                    "Unsupported operator {:?} on Number",
                    &expression.operator
                )),
            },
            ValueType::String => match &cel_value {
                Expression::Atom(Atom::String(_)) => Ok(cel_value),
                _ => Ok(Expression::Atom(Atom::String(Arc::new(
                    expression.value.clone(),
                )))),
            },
            ValueType::Bytes => match expression.operator {
                WhenConditionOperator::Equal | WhenConditionOperator::NotEqual => {
                    if let Expression::Atom(atom) = &cel_value {
                        match atom {
                            Atom::String(_str) => Ok(cel_value),
                            Atom::Bytes(_bytes) => Ok(cel_value),
                            _ => Err(format!("Can't compare {cel_value:?} with Bytes")),
                        }
                    } else {
                        Err(format!("Can't compare {cel_value:?} with Bytes"))
                    }
                }
                _ => Err(format!(
                    "Unsupported operator {:?} on Bytes",
                    &expression.operator
                )),
            },
            ValueType::Bool => match expression.operator {
                WhenConditionOperator::Equal | WhenConditionOperator::NotEqual => {
                    if let Expression::Atom(atom) = &cel_value {
                        match atom {
                            Atom::Bool(_) => Ok(cel_value),
                            _ => Err(format!("Can't compare {cel_value:?} with Bool")),
                        }
                    } else {
                        Err(format!("Can't compare {cel_value:?} with Bool"))
                    }
                }
                _ => Err(format!(
                    "Unsupported operator {:?} on Bool",
                    &expression.operator
                )),
            },
            ValueType::Timestamp => match expression.operator {
                WhenConditionOperator::Equal | WhenConditionOperator::NotEqual => {
                    if let Expression::Atom(atom) = &cel_value {
                        match atom {
                            Atom::String(_) => Ok(Expression::FunctionCall(
                                Expression::Ident("timestamp".to_string().into()).into(),
                                None,
                                [cel_value].to_vec(),
                            )),
                            _ => Err(format!("Can't compare {cel_value:?} with Timestamp")),
                        }
                    } else {
                        Err(format!("Can't compare {cel_value:?} with Bool"))
                    }
                }
                _ => Err(format!(
                    "Unsupported operator {:?} on Bytes",
                    &expression.operator
                )),
            },
            _ => Err(format!(
                "Still needs support for values of type `{cel_type}`"
            )),
        }?;

        let expression = match expression.operator {
            WhenConditionOperator::Equal => Expression::Relation(
                Expression::Ident(Arc::new("attribute".to_string())).into(),
                RelationOp::Equals,
                value.into(),
            ),
            WhenConditionOperator::NotEqual => Expression::Relation(
                Expression::Ident(Arc::new("attribute".to_string())).into(),
                RelationOp::NotEquals,
                value.into(),
            ),
            WhenConditionOperator::StartsWith => Expression::FunctionCall(
                Expression::Ident(Arc::new("startsWith".to_string())).into(),
                Some(Expression::Ident("attribute".to_string().into()).into()),
                [value].to_vec(),
            ),
            WhenConditionOperator::EndsWith => Expression::FunctionCall(
                Expression::Ident(Arc::new("endsWith".to_string())).into(),
                Some(Expression::Ident("attribute".to_string().into()).into()),
                [value].to_vec(),
            ),
            WhenConditionOperator::Matches => Expression::FunctionCall(
                Expression::Ident(Arc::new("matches".to_string())).into(),
                Some(Expression::Ident("attribute".to_string().into()).into()),
                [value].to_vec(),
            ),
        };

        Ok(Self {
            expression,
            cel_type,
        })
    }
}

pub fn type_of(path: &str) -> Option<ValueType> {
    match path {
        "request.time" => Some(ValueType::Timestamp),
        "request.id" => Some(ValueType::String),
        "request.protocol" => Some(ValueType::String),
        "request.scheme" => Some(ValueType::String),
        "request.host" => Some(ValueType::String),
        "request.method" => Some(ValueType::String),
        "request.path" => Some(ValueType::String),
        "request.url_path" => Some(ValueType::String),
        "request.query" => Some(ValueType::String),
        "request.referer" => Some(ValueType::String),
        "request.useragent" => Some(ValueType::String),
        "request.body" => Some(ValueType::String),
        "source.address" => Some(ValueType::String),
        "source.service" => Some(ValueType::String),
        "source.principal" => Some(ValueType::String),
        "source.certificate" => Some(ValueType::String),
        "destination.address" => Some(ValueType::String),
        "destination.service" => Some(ValueType::String),
        "destination.principal" => Some(ValueType::String),
        "destination.certificate" => Some(ValueType::String),
        "connection.requested_server_name" => Some(ValueType::String),
        "connection.tls_session.sni" => Some(ValueType::String),
        "connection.tls_version" => Some(ValueType::String),
        "connection.subject_local_certificate" => Some(ValueType::String),
        "connection.subject_peer_certificate" => Some(ValueType::String),
        "connection.dns_san_local_certificate" => Some(ValueType::String),
        "connection.dns_san_peer_certificate" => Some(ValueType::String),
        "connection.uri_san_local_certificate" => Some(ValueType::String),
        "connection.uri_san_peer_certificate" => Some(ValueType::String),
        "connection.sha256_peer_certificate_digest" => Some(ValueType::String),
        "ratelimit.domain" => Some(ValueType::String),
        "request.size" => Some(ValueType::Int),
        "source.port" => Some(ValueType::Int),
        "destination.port" => Some(ValueType::Int),
        "connection.id" => Some(ValueType::Int),
        "ratelimit.hits_addend" => Some(ValueType::Int),
        "request.headers" => Some(ValueType::Map),
        "request.context_extensions" => Some(ValueType::Map),
        "source.labels" => Some(ValueType::Map),
        "destination.labels" => Some(ValueType::Map),
        "filter_state" => Some(ValueType::Map),
        "connection.mtls" => Some(ValueType::Bool),
        "request.raw_body" => Some(ValueType::Bytes),
        "auth.identity" => Some(ValueType::Bytes),
        _ => None,
    }
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
            for pe in &action_set.route_rule_conditions.matches {
                let result = pe.compile();
                if result.is_err() {
                    return Err(result.err().unwrap());
                }
            }
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
                for condition in &action.conditions {
                    let result = condition.compile();
                    if result.is_err() {
                        return Err(result.err().unwrap());
                    }
                }
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
                "matches": [
                {
                    "selector": "request.path",
                    "operator": "eq",
                    "value": "/admin/toy"
                },
                {
                    "selector": "request.method",
                    "operator": "eq",
                    "value": "POST"
                },
                {
                    "selector": "request.host",
                    "operator": "eq",
                    "value": "cars.toystore.com"
                }]
            },
            "actions": [
            {
                "service": "authorino",
                "scope": "authconfig-A"
            },
            {
                "service": "limitador",
                "scope": "rlp-ns-A/rlp-name-A",
                "conditions": [
                {
                    "selector": "auth.metadata.username",
                    "operator": "eq",
                    "value": "alice"
                }],
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

        let matches = &filter_config.action_sets[0].route_rule_conditions.matches;
        assert_eq!(matches.len(), 3);

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

        let rl_conditions = &rl_action.conditions;
        assert_eq!(rl_conditions.len(), 1);

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
    fn parse_config_data_selector() {
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
                        "selector": {
                            "selector": "my.selector.path",
                            "key": "mykey",
                            "default": "my_selector_default_value"
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

        let actions = &filter_config.action_sets[0].actions;
        assert_eq!(actions.len(), 1);

        let data_items = &actions[0].data;
        assert_eq!(data_items.len(), 1);

        if let DataType::Selector(selector_item) = &data_items[0].item {
            assert_eq!(selector_item.selector, "my.selector.path");
            assert_eq!(selector_item.key.as_ref().unwrap(), "mykey");
            assert_eq!(
                selector_item.default.as_ref().unwrap(),
                "my_selector_default_value"
            );
        } else {
            panic!();
        }
    }

    #[test]
    fn parse_config_condition_selector_operators() {
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
                    "hostnames": ["*.toystore.com", "example.com"],
                    "matches": [
                    {
                        "selector": "request.path",
                        "operator": "eq",
                        "value": "/admin/toy"
                    },
                    {
                        "selector": "request.method",
                        "operator": "neq",
                        "value": "POST"
                    },
                    {
                        "selector": "request.host",
                        "operator": "startswith",
                        "value": "cars."
                    },
                    {
                        "selector": "request.host",
                        "operator": "endswith",
                        "value": ".com"
                    },
                    {
                        "selector": "request.host",
                        "operator": "matches",
                        "value": "*.com"
                    }]
                },
                "actions": [
                {
                    "service": "limitador",
                    "scope": "rlp-ns-A/rlp-name-A",
                    "conditions": [
                    {
                        "selector": "auth.metadata.username",
                        "operator": "eq",
                        "value": "alice"
                    },
                    {
                        "selector": "request.path",
                        "operator": "endswith",
                        "value": "/car"
                    }],
                    "data": [ { "selector": { "selector": "my.selector.path" } }]
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

        let matches = &filter_config.action_sets[0].route_rule_conditions.matches;
        assert_eq!(matches.len(), 5);

        let expected_matches = [
            // selector, value, operator
            ("request.path", "/admin/toy", WhenConditionOperator::Equal),
            ("request.method", "POST", WhenConditionOperator::NotEqual),
            ("request.host", "cars.", WhenConditionOperator::StartsWith),
            ("request.host", ".com", WhenConditionOperator::EndsWith),
            ("request.host", "*.com", WhenConditionOperator::Matches),
        ];

        for (m, (expected_selector, expected_value, expected_operator)) in
            matches.iter().zip(expected_matches.iter())
        {
            assert_eq!(m.selector, *expected_selector);
            assert_eq!(m.value, *expected_value);
            assert_eq!(m.operator, *expected_operator);
        }

        let conditions = &filter_config.action_sets[0].actions[0].conditions;
        assert_eq!(conditions.len(), 2);

        let expected_conditions = [
            // selector, value, operator
            (
                "auth.metadata.username",
                "alice",
                WhenConditionOperator::Equal,
            ),
            ("request.path", "/car", WhenConditionOperator::EndsWith),
        ];

        for (condition, (expected_selector, expected_value, expected_operator)) in
            conditions.iter().zip(expected_conditions.iter())
        {
            assert_eq!(condition.selector, *expected_selector);
            assert_eq!(condition.value, *expected_value);
            assert_eq!(condition.operator, *expected_operator);
        }
    }

    #[test]
    fn parse_config_conditions_optional() {
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
                        "selector": {
                            "selector": "auth.metadata.username"
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

        let matches = &filter_config.action_sets[0].route_rule_conditions.matches;
        assert_eq!(matches.len(), 0);
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
                    "selector": {
                        "selector": "auth.metadata.username"
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

        // condition selector operator unknown
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
                    "hostnames": ["*.toystore.com", "example.com"],
                    "matches": [
                    {
                        "selector": "request.path",
                        "operator": "unknown",
                        "value": "/admin/toy"
                    }]
                },
                "actions": [
                {
                    "service": "limitador",
                    "scope": "rlp-ns-A/rlp-name-A",
                    "data": [ { "selector": { "selector": "my.selector.path" } }]
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

    mod pattern_expressions {
        use crate::configuration::{PatternExpression, WhenConditionOperator};

        #[test]
        fn test_legacy_string() {
            let p = PatternExpression {
                selector: "request.id".to_string(),
                operator: WhenConditionOperator::Equal,
                value: "request_id".to_string(),
                path: Default::default(),
                compiled: Default::default(),
            };
            p.compile().expect("Should compile fine!");
            assert_eq!(
                p.eval("request_id".as_bytes().to_vec()),
                Ok(true),
                "Expression: {:?}",
                p.compiled.get()
            )
        }

        #[test]
        fn test_proper_string() {
            let p = PatternExpression {
                selector: "request.id".to_string(),
                operator: WhenConditionOperator::Equal,
                value: "\"request_id\"".to_string(),
                path: Default::default(),
                compiled: Default::default(),
            };
            p.compile().expect("Should compile fine!");
            assert_eq!(
                p.eval("request_id".as_bytes().to_vec()),
                Ok(true),
                "Expression: {:?}",
                p.compiled.get()
            )
        }

        #[test]
        fn test_proper_int_as_string() {
            let p = PatternExpression {
                selector: "request.id".to_string(),
                operator: WhenConditionOperator::Equal,
                value: "123".to_string(),
                path: Default::default(),
                compiled: Default::default(),
            };
            p.compile().expect("Should compile fine!");
            assert_eq!(
                p.eval("123".as_bytes().to_vec()),
                Ok(true),
                "Expression: {:?}",
                p.compiled.get()
            )
        }

        #[test]
        fn test_proper_string_inferred() {
            let p = PatternExpression {
                selector: "foobar".to_string(),
                operator: WhenConditionOperator::Equal,
                value: "\"123\"".to_string(),
                path: Default::default(),
                compiled: Default::default(),
            };
            p.compile().expect("Should compile fine!");
            assert_eq!(
                p.eval("123".as_bytes().to_vec()),
                Ok(true),
                "Expression: {:?}",
                p.compiled.get()
            )
        }

        #[test]
        fn test_int() {
            let p = PatternExpression {
                selector: "destination.port".to_string(),
                operator: WhenConditionOperator::Equal,
                value: "8080".to_string(),
                path: Default::default(),
                compiled: Default::default(),
            };
            p.compile().expect("Should compile fine!");
            assert_eq!(
                p.eval(8080_i64.to_le_bytes().to_vec()),
                Ok(true),
                "Expression: {:?}",
                p.compiled.get()
            )
        }

        #[test]
        fn test_int_inferred() {
            let p = PatternExpression {
                selector: "foobar".to_string(),
                operator: WhenConditionOperator::Equal,
                value: "8080".to_string(),
                path: Default::default(),
                compiled: Default::default(),
            };
            p.compile().expect("Should compile fine!");
            assert_eq!(
                p.eval(8080_i64.to_le_bytes().to_vec()),
                Ok(true),
                "Expression: {:?}",
                p.compiled.get()
            )
        }

        #[test]
        fn test_float_inferred() {
            let p = PatternExpression {
                selector: "foobar".to_string(),
                operator: WhenConditionOperator::Equal,
                value: "1.0".to_string(),
                path: Default::default(),
                compiled: Default::default(),
            };
            p.compile().expect("Should compile fine!");
            assert_eq!(
                p.eval(1_f64.to_le_bytes().to_vec()),
                Ok(true),
                "Expression: {:?}",
                p.compiled.get()
            )
        }

        #[test]
        fn test_bool() {
            let p = PatternExpression {
                selector: "connection.mtls".to_string(),
                operator: WhenConditionOperator::Equal,
                value: "true".to_string(),
                path: Default::default(),
                compiled: Default::default(),
            };
            p.compile().expect("Should compile fine!");
            assert_eq!(
                p.eval((true as u8).to_le_bytes().to_vec()),
                Ok(true),
                "Expression: {:?}",
                p.compiled.get()
            )
        }

        #[test]
        fn test_timestamp() {
            let p = PatternExpression {
                selector: "request.time".to_string(),
                operator: WhenConditionOperator::Equal,
                value: "2023-05-28T00:00:00+00:00".to_string(),
                path: Default::default(),
                compiled: Default::default(),
            };
            p.compile().expect("Should compile fine!");
            assert_eq!(
                p.eval(1685232000000000000_i64.to_le_bytes().to_vec()),
                Ok(true),
                "Expression: {:?}",
                p.compiled.get()
            )
        }
    }
}
