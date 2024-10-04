use std::cell::OnceCell;
use std::collections::HashMap;
use std::fmt::{Debug, Display, Formatter};
use std::rc::Rc;
use std::sync::Arc;

use crate::attribute::Attribute;
use crate::envoy::{RateLimitDescriptor, RateLimitDescriptor_Entry};
use crate::policy::Policy;
use crate::policy_index::PolicyIndex;
use crate::service::GrpcService;
use cel_interpreter::functions::duration;
use cel_interpreter::objects::ValueType;
use cel_interpreter::{Context, Expression, Value};
use cel_parser::{Atom, RelationOp};
use log::debug;
use protobuf::RepeatedField;
use proxy_wasm::hostcalls;
use serde::de::{Error, Visitor};
use serde::{Deserialize, Deserializer};
use std::time::Duration;

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
    path: OnceCell<Path>,
}

impl SelectorItem {
    pub fn compile(&self) -> Result<(), String> {
        self.path
            .set(self.selector.as_str().into())
            .map_err(|p| format!("Err on {p:?}"))
    }

    pub fn path(&self) -> &Path {
        self.path
            .get()
            .expect("SelectorItem wasn't previously compiled!")
    }
}

#[derive(Debug, Clone)]
pub struct Path {
    tokens: Vec<String>,
}

impl Display for Path {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            self.tokens
                .iter()
                .map(|t| t.replace('.', "\\."))
                .collect::<Vec<String>>()
                .join(".")
        )
    }
}

impl From<&str> for Path {
    fn from(value: &str) -> Self {
        let mut token = String::new();
        let mut tokens: Vec<String> = Vec::new();
        let mut chars = value.chars();
        while let Some(ch) = chars.next() {
            match ch {
                '.' => {
                    tokens.push(token);
                    token = String::new();
                }
                '\\' => {
                    if let Some(next) = chars.next() {
                        token.push(next);
                    }
                }
                _ => token.push(ch),
            }
        }
        tokens.push(token);

        Self { tokens }
    }
}

impl Path {
    pub fn tokens(&self) -> Vec<&str> {
        self.tokens.iter().map(String::as_str).collect()
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
}

impl DataType {
    pub fn compile(&self) -> Result<(), String> {
        match self {
            DataType::Static(_) => Ok(()),
            DataType::Selector(selector) => selector.compile(),
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
    path: OnceCell<Path>,
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
    pub fn path(&self) -> Vec<&str> {
        self.path
            .get()
            .expect("PatternExpression wasn't previously compiled!")
            .tokens()
    }

    pub fn eval(&self, raw_attribute: Vec<u8>) -> Result<bool, String> {
        let cel_type = &self.compiled.get().unwrap().cel_type;
        let value = match cel_type {
            ValueType::String => Value::String(Arc::new(Attribute::parse(raw_attribute)?)),
            ValueType::Int => Value::Int(Attribute::parse(raw_attribute)?),
            ValueType::UInt => Value::UInt(Attribute::parse(raw_attribute)?),
            ValueType::Float => Value::Float(Attribute::parse(raw_attribute)?),
            ValueType::Bytes => Value::Bytes(Arc::new(Attribute::parse(raw_attribute)?)),
            ValueType::Bool => Value::Bool(Attribute::parse(raw_attribute)?),
            ValueType::Timestamp => Value::Timestamp(Attribute::parse(raw_attribute)?),
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
    pub index: PolicyIndex,
    pub services: Rc<HashMap<String, Rc<GrpcService>>>,
}

impl Default for FilterConfig {
    fn default() -> Self {
        Self {
            index: PolicyIndex::new(),
            services: Rc::new(HashMap::new()),
        }
    }
}

impl TryFrom<PluginConfiguration> for FilterConfig {
    type Error = String;

    fn try_from(config: PluginConfiguration) -> Result<Self, Self::Error> {
        let mut index = PolicyIndex::new();

        for rlp in config.policies.iter() {
            for rule in &rlp.rules {
                for condition in &rule.conditions {
                    for pe in &condition.all_of {
                        let result = pe.compile();
                        if result.is_err() {
                            return Err(result.err().unwrap());
                        }
                    }
                }
                for action in &rule.actions {
                    for datum in &action.data {
                        let result = datum.item.compile();
                        if result.is_err() {
                            return Err(result.err().unwrap());
                        }
                    }
                }
            }

            for hostname in rlp.hostnames.iter() {
                index.insert(hostname, rlp.clone());
            }
        }

        // configure grpc services from the extensions in config
        let services = config
            .extensions
            .into_iter()
            .map(|(name, ext)| (name, Rc::new(GrpcService::new(Rc::new(ext)))))
            .collect();

        Ok(Self {
            index,
            services: Rc::new(services),
        })
    }
}

#[derive(Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum FailureMode {
    #[default]
    Deny,
    Allow,
}

#[derive(Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ExtensionType {
    Auth,
    #[default]
    RateLimit,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PluginConfiguration {
    pub extensions: HashMap<String, Extension>,
    pub policies: Vec<Policy>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Extension {
    #[serde(rename = "type")]
    pub extension_type: ExtensionType,
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

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub extension: String,
    pub scope: String,
    #[serde(default)]
    pub data: Vec<DataItem>,
}

impl Action {
    pub fn build_descriptors(&self) -> RepeatedField<RateLimitDescriptor> {
        let mut entries = RepeatedField::new();
        if let Some(desc) = self.build_single_descriptor() {
            entries.push(desc);
        }
        entries
    }

    fn build_single_descriptor(&self) -> Option<RateLimitDescriptor> {
        let mut entries = RepeatedField::default();

        // iterate over data items to allow any data item to skip the entire descriptor
        for data in self.data.iter() {
            match &data.item {
                DataType::Static(static_item) => {
                    let mut descriptor_entry = RateLimitDescriptor_Entry::new();
                    descriptor_entry.set_key(static_item.key.to_owned());
                    descriptor_entry.set_value(static_item.value.to_owned());
                    entries.push(descriptor_entry);
                }
                DataType::Selector(selector_item) => {
                    let descriptor_key = match &selector_item.key {
                        None => selector_item.path().to_string(),
                        Some(key) => key.to_owned(),
                    };

                    let attribute_path = selector_item.path();
                    debug!(
                        "get_property:  selector: {} path: {:?}",
                        selector_item.selector, attribute_path
                    );
                    let value = match hostcalls::get_property(attribute_path.tokens()).unwrap() {
                        //TODO(didierofrivia): Replace hostcalls by DI
                        None => {
                            debug!(
                                "build_single_descriptor: selector not found: {}",
                                attribute_path
                            );
                            match &selector_item.default {
                                None => return None, // skipping the entire descriptor
                                Some(default_value) => default_value.clone(),
                            }
                        }
                        // TODO(eastizle): not all fields are strings
                        // https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/advanced/attributes
                        Some(attribute_bytes) => match Attribute::parse(attribute_bytes) {
                            Ok(attr_str) => attr_str,
                            Err(e) => {
                                debug!("build_single_descriptor: failed to parse selector value: {}, error: {}",
                                    attribute_path, e);
                                return None;
                            }
                        },
                        // Alternative implementation (for rust >= 1.76)
                        // Attribute::parse(attribute_bytes)
                        //   .inspect_err(|e| debug!("#{} build_single_descriptor: failed to parse selector value: {}, error: {}",
                        //           filter.context_id, attribute_path, e))
                        //   .ok()?,
                    };
                    let mut descriptor_entry = RateLimitDescriptor_Entry::new();
                    descriptor_entry.set_key(descriptor_key);
                    descriptor_entry.set_value(value);
                    entries.push(descriptor_entry);
                }
            }
        }
        let mut res = RateLimitDescriptor::new();
        res.set_entries(entries);
        Some(res)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    const CONFIG: &str = r#"{
        "extensions": {
            "authorino": {
                "type": "auth",
                "endpoint": "authorino-cluster",
                "failureMode": "deny"
                "timeout": "24ms"
            },
            "limitador": {
                "type": "ratelimit",
                "endpoint": "limitador-cluster",
                "failureMode": "allow"
                "timeout": "42ms"
            }
        },
        "policies": [
        {
            "name": "rlp-ns-A/rlp-name-A",
            "hostnames": ["*.toystore.com", "example.com"],
            "rules": [
            {
                "conditions": [
                {
                   "allOf": [
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
                }],
                "actions": [
                {
                    "extension": "authorino",
                    "scope": "authconfig-A"
                },
                {
                    "extension": "limitador",
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
        assert_eq!(filter_config.policies.len(), 1);

        let extensions = &filter_config.extensions;
        assert_eq!(extensions.len(), 2);

        if let Some(auth_extension) = extensions.get("authorino") {
            assert_eq!(auth_extension.extension_type, ExtensionType::Auth);
            assert_eq!(auth_extension.endpoint, "authorino-cluster");
            assert_eq!(auth_extension.failure_mode, FailureMode::Deny);
            assert_eq!(auth_extension.timeout, Timeout(Duration::from_millis(24)));
        } else {
            panic!()
        }

        if let Some(rl_extension) = extensions.get("limitador") {
            assert_eq!(rl_extension.extension_type, ExtensionType::RateLimit);
            assert_eq!(rl_extension.endpoint, "limitador-cluster");
            assert_eq!(rl_extension.failure_mode, FailureMode::Allow);
            assert_eq!(rl_extension.timeout, Timeout(Duration::from_millis(42)));
        } else {
            panic!()
        }

        let rules = &filter_config.policies[0].rules;
        assert_eq!(rules.len(), 1);

        let conditions = &rules[0].conditions;
        assert_eq!(conditions.len(), 1);

        let all_of_conditions = &conditions[0].all_of;
        assert_eq!(all_of_conditions.len(), 3);

        let actions = &rules[0].actions;
        assert_eq!(actions.len(), 2);

        let auth_action = &actions[0];
        assert_eq!(auth_action.extension, "authorino");
        assert_eq!(auth_action.scope, "authconfig-A");

        let rl_action = &actions[1];
        assert_eq!(rl_action.extension, "limitador");
        assert_eq!(rl_action.scope, "rlp-ns-A/rlp-name-A");

        let auth_data_items = &auth_action.data;
        assert_eq!(auth_data_items.len(), 0);

        let rl_data_items = &rl_action.data;
        assert_eq!(rl_data_items.len(), 2);

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

        if let DataType::Selector(selector_item) = &rl_data_items[1].item {
            assert_eq!(selector_item.selector, "auth.metadata.username");
            assert!(selector_item.key.is_none());
            assert!(selector_item.default.is_none());
        } else {
            panic!();
        }
    }

    #[test]
    fn parse_config_min() {
        let config = r#"{
            "extensions": {},
            "policies": []
        }"#;
        let res = serde_json::from_str::<PluginConfiguration>(config);
        if let Err(ref e) = res {
            eprintln!("{e}");
        }
        assert!(res.is_ok());

        let filter_config = res.unwrap();
        assert_eq!(filter_config.policies.len(), 0);
    }

    #[test]
    fn parse_config_data_selector() {
        let config = r#"{
            "extensions": {
                "limitador": {
                    "type": "ratelimit",
                    "endpoint": "limitador-cluster",
                    "failureMode": "deny"
                }
            },
            "policies": [
            {
                "name": "rlp-ns-A/rlp-name-A",
                "hostnames": ["*.toystore.com", "example.com"],
                "rules": [
                {
                    "actions": [
                    {
                        "extension": "limitador",
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
            }]
        }"#;
        let res = serde_json::from_str::<PluginConfiguration>(config);
        if let Err(ref e) = res {
            eprintln!("{e}");
        }
        assert!(res.is_ok());

        let filter_config = res.unwrap();
        assert_eq!(filter_config.policies.len(), 1);

        let rules = &filter_config.policies[0].rules;
        assert_eq!(rules.len(), 1);

        let actions = &rules[0].actions;
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
            "extensions": {
                "limitador": {
                    "type": "ratelimit",
                    "endpoint": "limitador-cluster",
                    "failureMode": "deny"
                }
            },
            "policies": [
            {
                "name": "rlp-ns-A/rlp-name-A",
                "hostnames": ["*.toystore.com", "example.com"],
                "rules": [
                {
                    "conditions": [
                    {
                        "allOf": [
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
                    }],
                    "actions": [
                    {
                        "extension": "limitador",
                        "scope": "rlp-ns-A/rlp-name-A",
                        "data": [ { "selector": { "selector": "my.selector.path" } }]
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
        assert_eq!(filter_config.policies.len(), 1);

        let rules = &filter_config.policies[0].rules;
        assert_eq!(rules.len(), 1);

        let conditions = &rules[0].conditions;
        assert_eq!(conditions.len(), 1);

        let all_of_conditions = &conditions[0].all_of;
        assert_eq!(all_of_conditions.len(), 5);

        let expected_conditions = [
            // selector, value, operator
            ("request.path", "/admin/toy", WhenConditionOperator::Equal),
            ("request.method", "POST", WhenConditionOperator::NotEqual),
            ("request.host", "cars.", WhenConditionOperator::StartsWith),
            ("request.host", ".com", WhenConditionOperator::EndsWith),
            ("request.host", "*.com", WhenConditionOperator::Matches),
        ];

        for i in 0..expected_conditions.len() {
            assert_eq!(all_of_conditions[i].selector, expected_conditions[i].0);
            assert_eq!(all_of_conditions[i].value, expected_conditions[i].1);
            assert_eq!(all_of_conditions[i].operator, expected_conditions[i].2);
        }
    }

    #[test]
    fn parse_config_conditions_optional() {
        let config = r#"{
            "extensions": {
                "limitador": {
                    "type": "ratelimit",
                    "endpoint": "limitador-cluster",
                    "failureMode": "deny"
                }
            },
            "policies": [
            {
                "name": "rlp-ns-A/rlp-name-A",
                "hostnames": ["*.toystore.com", "example.com"],
                "rules": [
                {
                    "actions": [
                    {
                        "extension": "limitador",
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
            }]
        }"#;
        let res = serde_json::from_str::<PluginConfiguration>(config);
        if let Err(ref e) = res {
            eprintln!("{e}");
        }
        assert!(res.is_ok());

        let filter_config = res.unwrap();
        assert_eq!(filter_config.policies.len(), 1);

        let extensions = &filter_config.extensions;
        assert_eq!(
            extensions.get("limitador").unwrap().timeout,
            Timeout(Duration::from_millis(20))
        );

        let rules = &filter_config.policies[0].rules;
        assert_eq!(rules.len(), 1);

        let conditions = &rules[0].conditions;
        assert_eq!(conditions.len(), 0);
    }

    #[test]
    fn parse_config_invalid_data() {
        // data item fields are mutually exclusive
        let bad_config = r#"{
        "extensions": {
            "limitador": {
                "type": "ratelimit",
                "endpoint": "limitador-cluster",
                "failureMode": "deny"
            }
        },
        "policies": [
        {
            "name": "rlp-ns-A/rlp-name-A",
            "hostnames": ["*.toystore.com", "example.com"],
            "rules": [
            {
                "actions": [
                {
                    "extension": "limitador",
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
        }]
        }"#;
        let res = serde_json::from_str::<PluginConfiguration>(bad_config);
        assert!(res.is_err());

        // data item unknown fields are forbidden
        let bad_config = r#"{
        "extensions": {
            "limitador": {
                "type": "ratelimit",
                "endpoint": "limitador-cluster",
                "failureMode": "deny"
            }
        },
        "policies": [
        {
            "name": "rlp-ns-A/rlp-name-A",
            "service": "limitador-cluster",
            "hostnames": ["*.toystore.com", "example.com"],
            "rules": [
            {
                "actions": [
                {
                    "extension": "limitador",
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
        }]
        }"#;
        let res = serde_json::from_str::<PluginConfiguration>(bad_config);
        assert!(res.is_err());

        // condition selector operator unknown
        let bad_config = r#"{
            "extensions": {
                "limitador": {
                    "type": "ratelimit",
                    "endpoint": "limitador-cluster",
                    "failureMode": "deny"
                }
            },
            "policies": [
            {
                "name": "rlp-ns-A/rlp-name-A",
                "hostnames": ["*.toystore.com", "example.com"],
                "rules": [
                {
                    "conditions": [
                    {
                       "allOf": [
                        {
                            "selector": "request.path",
                            "operator": "unknown",
                            "value": "/admin/toy"
                        }]
                    }],
                    "actions": [
                    {
                        "extension": "limitador",
                        "scope": "rlp-ns-A/rlp-name-A",
                        "data": [ { "selector": { "selector": "my.selector.path" } }]
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
        let rlp_option = filter_config.index.get_longest_match_policy("example.com");
        assert!(rlp_option.is_some());

        let rlp_option = filter_config
            .index
            .get_longest_match_policy("test.toystore.com");
        assert!(rlp_option.is_some());

        let rlp_option = filter_config.index.get_longest_match_policy("unknown");
        assert!(rlp_option.is_none());
    }

    #[test]
    fn path_tokenizes_with_escaping_basic() {
        let path: Path = r"one\.two..three\\\\.four\\\.\five.".into();
        assert_eq!(
            path.tokens(),
            vec!["one.two", "", r"three\\", r"four\.five", ""]
        );
    }

    #[test]
    fn path_tokenizes_with_escaping_ends_with_separator() {
        let path: Path = r"one.".into();
        assert_eq!(path.tokens(), vec!["one", ""]);
    }

    #[test]
    fn path_tokenizes_with_escaping_ends_with_escape() {
        let path: Path = r"one\".into();
        assert_eq!(path.tokens(), vec!["one"]);
    }

    #[test]
    fn path_tokenizes_with_escaping_starts_with_separator() {
        let path: Path = r".one".into();
        assert_eq!(path.tokens(), vec!["", "one"]);
    }

    #[test]
    fn path_tokenizes_with_escaping_starts_with_escape() {
        let path: Path = r"\one".into();
        assert_eq!(path.tokens(), vec!["one"]);
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
