use crate::policy_index::PolicyIndex;
use cel_interpreter::objects::ValueType;
use cel_interpreter::{Context, Expression, Value};
use cel_parser::{Atom, RelationOp};
use serde::Deserialize;
use std::cell::OnceCell;
use std::fmt::{Display, Formatter};
use std::sync::Arc;

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
    pub fn compile(&self) -> Result<(), ()> {
        self.path.set(self.selector.as_str().into()).map_err(|_| ())
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
    pub fn compile(&self) -> Result<(), ()> {
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
    compiled: OnceCell<Expression>,
}

impl PatternExpression {
    pub fn compile(&self) -> Result<(), ()> {
        self.path
            .set(self.selector.as_str().into())
            .map_err(|_| ())?;
        self.compiled.set(self.try_into()?).map_err(|_| ())
    }
    pub fn path(&self) -> Vec<&str> {
        self.path
            .get()
            .expect("PatternExpression wasn't previously compiled!")
            .tokens()
    }

    pub fn expression(&self) -> &Expression {
        self.compiled
            .get()
            .expect("PatternExpression wasn't previously compiled!")
    }

    pub fn eval(&self, attribute_value: String) -> bool {
        let cel_type = type_of(&self.selector);
        let value = match cel_type {
            ValueType::String => Value::String(attribute_value.into()),
            ValueType::Int | ValueType::UInt => Value::Int(attribute_value.parse::<i64>().unwrap()),
            _ => unimplemented!("Need support for {}", cel_type),
        };
        let mut ctx = Context::default();
        ctx.add_variable("attribute", value).unwrap();
        Value::resolve(self.compiled.get().unwrap(), &ctx)
            .map(|v| {
                if let Value::Bool(result) = v {
                    result
                } else {
                    false
                }
            })
            .unwrap_or(false)
    }
}

impl TryFrom<&PatternExpression> for Expression {
    type Error = ();

    fn try_from(expression: &PatternExpression) -> Result<Self, Self::Error> {
        let cel_type = type_of(&expression.selector);

        let value = match cel_type {
            ValueType::Map => match expression.operator {
                WhenConditionOperator::Equal | WhenConditionOperator::NotEqual => {
                    match cel_parser::parse(&expression.value) {
                        Ok(exp) => {
                            if let Expression::Map(data) = exp {
                                Ok(Expression::Map(data))
                            } else {
                                Err(())
                            }
                        }
                        Err(_) => Err(()),
                    }
                }
                _ => Err(()),
            },
            ValueType::Int | ValueType::UInt => match expression.operator {
                WhenConditionOperator::Equal | WhenConditionOperator::NotEqual => {
                    match cel_parser::parse(&expression.value) {
                        Ok(exp) => {
                            if let Expression::Atom(atom) = &exp {
                                match atom {
                                    Atom::Int(_) | Atom::UInt(_) | Atom::Float(_) => Ok(exp),
                                    _ => Err(()),
                                }
                            } else {
                                Err(())
                            }
                        }
                        Err(_) => Err(()),
                    }
                }
                _ => Err(()),
            },
            ValueType::String => match expression.operator {
                WhenConditionOperator::Equal | WhenConditionOperator::NotEqual => Ok(
                    Expression::Atom(Atom::String(Arc::new(expression.value.clone()))),
                ),
                // WhenConditionOperator::Matches => {}
                _ => Ok(Expression::Atom(Atom::String(Arc::new(
                    expression.value.clone(),
                )))),
            },
            // ValueType::Bytes => {}
            // ValueType::Bool => {}
            // ValueType::Timestamp => {}
            _ => todo!("Still needs support for values of type `{cel_type}`"),
        }?;

        match expression.operator {
            WhenConditionOperator::Equal => Ok(Expression::Relation(
                Expression::Ident(Arc::new("attribute".to_string())).into(),
                RelationOp::Equals,
                value.into(),
            )),
            WhenConditionOperator::NotEqual => Ok(Expression::Relation(
                Expression::Ident(Arc::new("attribute".to_string())).into(),
                RelationOp::NotEquals,
                value.into(),
            )),
            WhenConditionOperator::StartsWith => Ok(Expression::FunctionCall(
                Expression::Ident(Arc::new("startsWith".to_string())).into(),
                Some(Expression::Ident("attribute".to_string().into()).into()),
                [value].to_vec(),
            )),
            WhenConditionOperator::EndsWith => Ok(Expression::FunctionCall(
                Expression::Ident(Arc::new("endsWith".to_string())).into(),
                Some(Expression::Ident("attribute".to_string().into()).into()),
                [value].to_vec(),
            )),
            WhenConditionOperator::Matches => Ok(Expression::FunctionCall(
                Expression::Ident(Arc::new("matches".to_string())).into(),
                Some(Expression::Ident("attribute".to_string().into()).into()),
                [value].to_vec(),
            )),
        }
    }
}

pub fn type_of(path: &str) -> ValueType {
    match path {
        "request.time" => ValueType::Timestamp,
        "request.id" => ValueType::String,
        "request.protocol" => ValueType::String,
        "request.scheme" => ValueType::String,
        "request.host" => ValueType::String,
        "request.method" => ValueType::String,
        "request.path" => ValueType::String,
        "request.url_path" => ValueType::String,
        "request.query" => ValueType::String,
        "request.referer" => ValueType::String,
        "request.useragent" => ValueType::String,
        "request.body" => ValueType::String,
        "source.address" => ValueType::String,
        "source.service" => ValueType::String,
        "source.principal" => ValueType::String,
        "source.certificate" => ValueType::String,
        "destination.address" => ValueType::String,
        "destination.service" => ValueType::String,
        "destination.principal" => ValueType::String,
        "destination.certificate" => ValueType::String,
        "connection.requested_server_name" => ValueType::String,
        "connection.tls_session.sni" => ValueType::String,
        "connection.tls_version" => ValueType::String,
        "connection.subject_local_certificate" => ValueType::String,
        "connection.subject_peer_certificate" => ValueType::String,
        "connection.dns_san_local_certificate" => ValueType::String,
        "connection.dns_san_peer_certificate" => ValueType::String,
        "connection.uri_san_local_certificate" => ValueType::String,
        "connection.uri_san_peer_certificate" => ValueType::String,
        "connection.sha256_peer_certificate_digest" => ValueType::String,
        "ratelimit.domain" => ValueType::String,
        "request.size" => ValueType::Int,
        "source.port" => ValueType::Int,
        "destination.port" => ValueType::Int,
        "connection.id" => ValueType::Int,
        "ratelimit.hits_addend" => ValueType::Int,
        "request.headers" => ValueType::Map,
        "request.context_extensions" => ValueType::Map,
        "source.labels" => ValueType::Map,
        "destination.labels" => ValueType::Map,
        "filter_state" => ValueType::Map,
        "connection.mtls" => ValueType::Bool,
        "request.raw_body" => ValueType::Bytes,
        "auth.identity" => ValueType::Bytes,
        _ => ValueType::Bytes,
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Condition {
    pub all_of: Vec<PatternExpression>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Rule {
    //
    #[serde(default)]
    pub conditions: Vec<Condition>,
    //
    pub data: Vec<DataItem>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitPolicy {
    pub name: String,
    pub domain: String,
    pub service: String,
    pub hostnames: Vec<String>,
    pub rules: Vec<Rule>,
}

impl RateLimitPolicy {
    #[cfg(test)]
    pub fn new(
        name: String,
        domain: String,
        service: String,
        hostnames: Vec<String>,
        rules: Vec<Rule>,
    ) -> Self {
        RateLimitPolicy {
            name,
            domain,
            service,
            hostnames,
            rules,
        }
    }
}

pub struct FilterConfig {
    pub index: PolicyIndex,
    // Deny/Allow request when faced with an irrecoverable failure.
    pub failure_mode: FailureMode,
}

impl Default for FilterConfig {
    fn default() -> Self {
        Self {
            index: PolicyIndex::new(),
            failure_mode: FailureMode::Deny,
        }
    }
}

impl TryFrom<PluginConfiguration> for FilterConfig {
    type Error = ();

    fn try_from(config: PluginConfiguration) -> Result<Self, Self::Error> {
        let mut index = PolicyIndex::new();

        for rlp in config.rate_limit_policies.iter() {
            for rule in &rlp.rules {
                for datum in &rule.data {
                    if datum.item.compile().is_err() {
                        return Err(());
                    }
                }
                for condition in &rule.conditions {
                    for pe in &condition.all_of {
                        if pe.compile().is_err() {
                            return Err(());
                        }
                    }
                }
            }
            for hostname in rlp.hostnames.iter() {
                index.insert(hostname, rlp.clone());
            }
        }

        Ok(Self {
            index,
            failure_mode: config.failure_mode,
        })
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "lowercase")]
pub enum FailureMode {
    Deny,
    Allow,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PluginConfiguration {
    pub rate_limit_policies: Vec<RateLimitPolicy>,
    // Deny/Allow request when faced with an irrecoverable failure.
    pub failure_mode: FailureMode,
}

#[cfg(test)]
mod test {
    use super::*;

    const CONFIG: &str = r#"{
        "failureMode": "deny",
        "rateLimitPolicies": [
        {
            "name": "rlp-ns-A/rlp-name-A",
            "domain": "rlp-ns-A/rlp-name-A",
            "service": "limitador-cluster",
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

    #[test]
    fn parse_config_happy_path() {
        let res = serde_json::from_str::<PluginConfiguration>(CONFIG);
        if let Err(ref e) = res {
            eprintln!("{e}");
        }
        assert!(res.is_ok());

        let filter_config = res.unwrap();
        assert_eq!(filter_config.rate_limit_policies.len(), 1);

        let rules = &filter_config.rate_limit_policies[0].rules;
        assert_eq!(rules.len(), 1);

        let conditions = &rules[0].conditions;
        assert_eq!(conditions.len(), 1);

        let all_of_conditions = &conditions[0].all_of;
        assert_eq!(all_of_conditions.len(), 3);

        let data_items = &rules[0].data;
        assert_eq!(data_items.len(), 2);

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

        if let DataType::Static(static_item) = &data_items[0].item {
            assert_eq!(static_item.key, "rlp-ns-A/rlp-name-A");
            assert_eq!(static_item.value, "1");
        } else {
            panic!();
        }

        if let DataType::Selector(selector_item) = &data_items[1].item {
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
            "failureMode": "deny",
            "rateLimitPolicies": []
        }"#;
        let res = serde_json::from_str::<PluginConfiguration>(config);
        if let Err(ref e) = res {
            eprintln!("{e}");
        }
        assert!(res.is_ok());

        let filter_config = res.unwrap();
        assert_eq!(filter_config.rate_limit_policies.len(), 0);
    }

    #[test]
    fn parse_config_data_selector() {
        let config = r#"{
            "failureMode": "deny",
            "rateLimitPolicies": [
            {
                "name": "rlp-ns-A/rlp-name-A",
                "domain": "rlp-ns-A/rlp-name-A",
                "service": "limitador-cluster",
                "hostnames": ["*.toystore.com", "example.com"],
                "rules": [
                {
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
        assert_eq!(filter_config.rate_limit_policies.len(), 1);

        let rules = &filter_config.rate_limit_policies[0].rules;
        assert_eq!(rules.len(), 1);

        let data_items = &rules[0].data;
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
            "failureMode": "deny",
            "rateLimitPolicies": [
            {
                "name": "rlp-ns-A/rlp-name-A",
                "domain": "rlp-ns-A/rlp-name-A",
                "service": "limitador-cluster",
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
        assert_eq!(filter_config.rate_limit_policies.len(), 1);

        let rules = &filter_config.rate_limit_policies[0].rules;
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
            "failureMode": "deny",
            "rateLimitPolicies": [
            {
                "name": "rlp-ns-A/rlp-name-A",
                "domain": "rlp-ns-A/rlp-name-A",
                "service": "limitador-cluster",
                "hostnames": ["*.toystore.com", "example.com"],
                "rules": [
                {
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
        assert_eq!(filter_config.rate_limit_policies.len(), 1);

        let rules = &filter_config.rate_limit_policies[0].rules;
        assert_eq!(rules.len(), 1);

        let conditions = &rules[0].conditions;
        assert_eq!(conditions.len(), 0);
    }

    #[test]
    fn parse_config_invalid_data() {
        // data item fields are mutually exclusive
        let bad_config = r#"{
        "failureMode": "deny",
        "rateLimitPolicies": [
        {
            "name": "rlp-ns-A/rlp-name-A",
            "domain": "rlp-ns-A/rlp-name-A",
            "service": "limitador-cluster",
            "hostnames": ["*.toystore.com", "example.com"],
            "rules": [
            {
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
        "failureMode": "deny",
        "rateLimitPolicies": [
        {
            "name": "rlp-ns-A/rlp-name-A",
            "domain": "rlp-ns-A/rlp-name-A",
            "service": "limitador-cluster",
            "hostnames": ["*.toystore.com", "example.com"],
            "rules": [
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
        }"#;
        let res = serde_json::from_str::<PluginConfiguration>(bad_config);
        assert!(res.is_err());

        // condition selector operator unknown
        let bad_config = r#"{
            "failureMode": "deny",
            "rateLimitPolicies": [
            {
                "name": "rlp-ns-A/rlp-name-A",
                "domain": "rlp-ns-A/rlp-name-A",
                "service": "limitador-cluster",
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

        let filter_config = FilterConfig::try_from(res.unwrap()).expect("That didn't work");
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
}
