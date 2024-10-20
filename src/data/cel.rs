use crate::data::get_attribute;
use crate::data::property::Path;
use cel_interpreter::objects::{Key, ValueType};
use cel_interpreter::{Context, Value};
use chrono::{DateTime, FixedOffset};
use std::collections::HashMap;
use std::sync::OnceLock;
use cel_parser::{parse, Expression as CelExpression, ParseError, Member};

pub struct Expression {
    attributes: Vec<Attribute>,
    expression: CelExpression,
}

impl Expression {
    pub fn new(expression: &str) -> Result<Self, ParseError> {
        let expression = parse(expression)?;

        let mut props = Vec::with_capacity(5);
        properties(&expression, &mut props, &mut Vec::default());

        Ok(Self {
            attributes: props.into_iter().map(|tokens| know_attribute_for(&Path::new(tokens)).expect("Unknown attribute")).collect(),
            expression,
        })
    }

    pub fn eval(&self) -> Value {
        let _data = self.build_data_map();
        let ctx = Context::default();
        // ctx.add_variable_from_value::<_, Map>("request", data.remove("request").unwrap_or_default().into());
        // ctx.add_variable_from_value::<_, Value::Map>("metadata", data.remove("metadata").unwrap_or_default());
        // ctx.add_variable_from_value::<_, Value::Map>("source", data.remove("source").unwrap_or_default());
        // ctx.add_variable_from_value::<_, Value::Map>("destination", data.remove("destination").unwrap_or_default().into());
        // ctx.add_variable_from_value::<_, Value::Map>("auth", data.remove("auth").unwrap_or_default().into());
        Value::resolve(&self.expression, &ctx).expect("Cel expression couldn't be evaluated")
    }

    fn build_data_map(&self) -> HashMap<String, HashMap<Key, Value>> {
        HashMap::default()
    }
}

pub struct Predicate {
    expression: Expression,
}

impl Predicate {
    pub fn test(&self) -> bool {
        match self.expression.eval() {
            Value::Bool(result) => result,
            _ => false,
        }
    }
}

pub struct Attribute {
    path: Path,
    cel_type: ValueType,
}

impl Attribute {
    pub fn get(&self) -> Value {
        match self.cel_type {
            ValueType::String => get_attribute::<String>(&self.path)
                .expect("Failed getting to known attribute")
                .map(|v| Value::String(v.into()))
                .unwrap_or(Value::Null),
            ValueType::Int => get_attribute::<i64>(&self.path)
                .expect("Failed getting to known attribute")
                .map(Value::Int)
                .unwrap_or(Value::Null),
            ValueType::UInt => get_attribute::<u64>(&self.path)
                .expect("Failed getting to known attribute")
                .map(Value::UInt)
                .unwrap_or(Value::Null),
            ValueType::Float => get_attribute::<f64>(&self.path)
                .expect("Failed getting to known attribute")
                .map(Value::Float)
                .unwrap_or(Value::Null),
            ValueType::Bool => get_attribute::<bool>(&self.path)
                .expect("Failed getting to known attribute")
                .map(Value::Bool)
                .unwrap_or(Value::Null),
            ValueType::Bytes => get_attribute::<Vec<u8>>(&self.path)
                .expect("Failed getting to known attribute")
                .map(|v| Value::Bytes(v.into()))
                .unwrap_or(Value::Null),
            ValueType::Timestamp => get_attribute::<DateTime<FixedOffset>>(&self.path)
                .expect("Failed getting to known attribute")
                .map(Value::Timestamp)
                .unwrap_or(Value::Null),
            _ => todo!("Need support for `{}`s!", self.cel_type),
        }
    }
}

pub fn know_attribute_for(path: &Path) -> Option<Attribute> {
    static WELL_KNOWN_ATTTRIBUTES: OnceLock<HashMap<Path, ValueType>> = OnceLock::new();
    WELL_KNOWN_ATTTRIBUTES
        .get_or_init(new_well_known_attribute_map)
        .get(path)
        .map(|t| Attribute {
            path: path.clone(),
            cel_type: copy(t),
        })
}

fn copy(value_type: &ValueType) -> ValueType {
    match value_type {
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
    }
}

fn new_well_known_attribute_map() -> HashMap<Path, ValueType> {
    HashMap::from([
        ("request.time".into(), ValueType::String),
        ("request.id".into(), ValueType::String),
        ("request.protocol".into(), ValueType::String),
        ("request.scheme".into(), ValueType::String),
        ("request.host".into(), ValueType::String),
        ("request.method".into(), ValueType::String),
        ("request.path".into(), ValueType::String),
        ("request.url_path".into(), ValueType::String),
        ("request.query".into(), ValueType::String),
        ("request.referer".into(), ValueType::String),
        ("request.size".into(), ValueType::Int),
        ("request.useragent".into(), ValueType::String),
        ("request.body".into(), ValueType::String),
        ("source.address".into(), ValueType::String),
        ("source.remote_address".into(), ValueType::String),
        ("source.port".into(), ValueType::Int),
        ("source.service".into(), ValueType::String),
        ("source.principal".into(), ValueType::String),
        ("source.certificate".into(), ValueType::String),
        ("destination.address".into(), ValueType::String),
        ("destination.port".into(), ValueType::Int),
        ("destination.service".into(), ValueType::String),
        ("destination.principal".into(), ValueType::String),
        ("destination.certificate".into(), ValueType::String),
        ("connection.requested_server_name".into(), ValueType::String),
        ("connection.tls_session.sni".into(), ValueType::String),
        ("connection.tls_version".into(), ValueType::String),
        (
            "connection.subject_local_certificate".into(),
            ValueType::String,
        ),
        (
            "connection.subject_peer_certificate".into(),
            ValueType::String,
        ),
        (
            "connection.dns_san_local_certificate".into(),
            ValueType::String,
        ),
        (
            "connection.dns_san_peer_certificate".into(),
            ValueType::String,
        ),
        (
            "connection.uri_san_local_certificate".into(),
            ValueType::String,
        ),
        (
            "connection.uri_san_peer_certificate".into(),
            ValueType::String,
        ),
        (
            "connection.sha256_peer_certificate_digest".into(),
            ValueType::String,
        ),
        ("ratelimit.domain".into(), ValueType::String),
        ("connection.id".into(), ValueType::Int),
        ("ratelimit.hits_addend".into(), ValueType::Int),
        ("request.headers".into(), ValueType::Map),
        ("request.context_extensions".into(), ValueType::Map),
        ("source.labels".into(), ValueType::Map),
        ("destination.labels".into(), ValueType::Map),
        ("filter_state".into(), ValueType::Map),
        ("connection.mtls".into(), ValueType::Bool),
        ("request.raw_body".into(), ValueType::Bytes),
        ("auth.identity".into(), ValueType::Bytes),
    ])
}

fn properties<'e>(
    exp: &'e CelExpression,
    all: &mut Vec<Vec<&'e str>>,
    path: &mut Vec<&'e str>,
) {
    match exp {
        CelExpression::Arithmetic(e1, _, e2)
        | CelExpression::Relation(e1, _, e2)
        | CelExpression::Ternary(e1, _, e2)
        | CelExpression::Or(e1, e2)
        | CelExpression::And(e1, e2) => {
            properties(e1, all, path);
            properties(e2, all, path);
        }
        CelExpression::Unary(_, e) => {
            properties(e, all, path);
        }
        CelExpression::Member(e, a) => {
            if let Member::Attribute(attr) = &**a {
                path.insert(0, attr.as_str())
            }
            properties(e, all, path);
        }
        CelExpression::FunctionCall(_, target, args) => {
            if let Some(target) = target {
                properties(target, all, path);
            }
            for e in args {
                properties(e, all, path);
            }
        }
        CelExpression::List(e) => {
            for e in e {
                properties(e, all, path);
            }
        }
        CelExpression::Map(v) => {
            for (e1, e2) in v {
                properties(e1, all, path);
                properties(e2, all, path);
            }
        }
        CelExpression::Atom(_) => {}
        CelExpression::Ident(v) => {
            if !path.is_empty() {
                path.insert(0, v.as_str());
                all.push(path.clone());
                path.clear();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cel_interpreter::objects::ValueType;

    #[test]
    fn finds_known_attributes() {
        let path = "request.method".into();
        let attr = know_attribute_for(&path).expect("Must be a hit!");
        assert_eq!(attr.path, path);
        match attr.cel_type {
            ValueType::String => {}
            _ => assert!(false),
        }
    }
}
