use crate::data::get_attribute;
use crate::data::property::{host_get_map, Path};
use cel_interpreter::objects::{Map, ValueType};
use cel_interpreter::{Context, Value};
use cel_parser::{parse, Expression as CelExpression, Member, ParseError};
use chrono::{DateTime, FixedOffset};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::sync::OnceLock;

#[derive(Clone, Debug)]
pub struct Expression {
    attributes: Vec<Attribute>,
    expression: CelExpression,
}

impl Expression {
    pub fn new(expression: &str) -> Result<Self, ParseError> {
        let expression = parse(expression)?;

        let mut props = Vec::with_capacity(5);
        properties(&expression, &mut props, &mut Vec::default());

        let mut attributes: Vec<Attribute> = props
            .into_iter()
            .map(|tokens| {
                let path = Path::new(tokens);
                known_attribute_for(&path).unwrap_or(Attribute {
                    path,
                    cel_type: None,
                })
            })
            .collect();

        attributes.sort_by(|a, b| a.path.tokens().len().cmp(&b.path.tokens().len()));

        Ok(Self {
            attributes,
            expression,
        })
    }

    pub fn eval(&self) -> Value {
        let mut ctx = Context::default();
        let Map { map } = self.build_data_map();

        // if expression was "auth.identity.anonymous",
        // {
        //   "auth": { "identity": { "anonymous": true } }
        // }
        for binding in ["request", "metadata", "source", "destination", "auth"] {
            ctx.add_variable_from_value(
                binding,
                map.get(&binding.into()).cloned().unwrap_or(Value::Null),
            );
        }
        Value::resolve(&self.expression, &ctx).expect("Cel expression couldn't be evaluated")
    }

    fn build_data_map(&self) -> Map {
        data::AttributeMap::new(self.attributes.clone()).into()
    }
}

#[derive(Clone, Debug)]
pub struct Predicate {
    expression: Expression,
}

impl Predicate {
    pub fn new(predicate: &str) -> Result<Self, ParseError> {
        Ok(Self {
            expression: Expression::new(predicate)?,
        })
    }

    pub fn test(&self) -> bool {
        match self.expression.eval() {
            Value::Bool(result) => result,
            _ => false,
        }
    }
}

pub struct Attribute {
    path: Path,
    cel_type: Option<ValueType>,
}

impl Debug for Attribute {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Attribute {{ {:?} }}", self.path)
    }
}

impl Clone for Attribute {
    fn clone(&self) -> Self {
        Attribute {
            path: self.path.clone(),
            cel_type: self.cel_type.as_ref().map(copy),
        }
    }
}

impl Attribute {
    pub fn get(&self) -> Value {
        match &self.cel_type {
            Some(t) => match t {
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
                ValueType::Map => host_get_map(&self.path)
                    .map(cel_interpreter::objects::Map::from)
                    .map(Value::Map)
                    .unwrap_or(Value::Null),
                _ => todo!("Need support for `{t}`s!"),
            },
            None => match get_attribute::<String>(&self.path).expect("Path must resolve!") {
                None => Value::Null,
                Some(json) => json_to_cel(&json),
            },
        }
    }
}

pub fn known_attribute_for(path: &Path) -> Option<Attribute> {
    static WELL_KNOWN_ATTRIBUTES: OnceLock<HashMap<Path, ValueType>> = OnceLock::new();
    WELL_KNOWN_ATTRIBUTES
        .get_or_init(new_well_known_attribute_map)
        .get(path)
        .map(|t| Attribute {
            path: path.clone(),
            cel_type: Some(copy(t)),
        })
}

fn json_to_cel(json: &str) -> Value {
    let json_value: Result<JsonValue, _> = serde_json::from_str(json);
    match json_value {
        Ok(json) => match json {
            JsonValue::Null => Value::Null,
            JsonValue::Bool(b) => b.into(),
            JsonValue::Number(n) => {
                if n.is_u64() {
                    n.as_u64().unwrap().into()
                } else if n.is_i64() {
                    n.as_i64().unwrap().into()
                } else {
                    n.as_f64().unwrap().into()
                }
            }
            JsonValue::String(str) => str.into(),
            _ => todo!("Need support for more Json!"),
        },
        _ => json.into(),
    }
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
        ("request.time".into(), ValueType::Timestamp),
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
    ])
}

fn properties<'e>(exp: &'e CelExpression, all: &mut Vec<Vec<&'e str>>, path: &mut Vec<&'e str>) {
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

pub mod data {
    use crate::data::cel::Attribute;
    use cel_interpreter::objects::{Key, Map};
    use cel_interpreter::Value;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[derive(Debug)]
    enum Token {
        Node(HashMap<String, Token>),
        Value(Attribute),
    }

    pub struct AttributeMap {
        data: HashMap<String, Token>,
    }

    impl AttributeMap {
        pub fn new(attributes: Vec<Attribute>) -> Self {
            let mut root = HashMap::default();
            for attr in attributes {
                let mut node = &mut root;
                let mut it = attr.path.tokens().into_iter();
                while let Some(token) = it.next() {
                    if it.len() != 0 {
                        node = match node
                            .entry(token.to_string())
                            .or_insert_with(|| Token::Node(HashMap::default()))
                        {
                            Token::Node(node) => node,
                            // a value was installed, on this path...
                            // so that value should resolve from there on
                            Token::Value(_) => break,
                        };
                    } else {
                        node.insert(token.to_string(), Token::Value(attr.clone()));
                    }
                }
            }
            Self { data: root }
        }
    }

    impl From<AttributeMap> for Map {
        fn from(value: AttributeMap) -> Self {
            map_to_value(value.data)
        }
    }

    fn map_to_value(map: HashMap<String, Token>) -> Map {
        let mut out: HashMap<Key, Value> = HashMap::default();
        for (key, value) in map {
            let k = key.into();
            let v = match value {
                Token::Value(v) => v.get(),
                Token::Node(map) => Value::Map(map_to_value(map)),
            };
            out.insert(k, v);
        }
        Map { map: Arc::new(out) }
    }

    #[cfg(test)]
    mod tests {
        use crate::data::cel::data::{AttributeMap, Token};
        use crate::data::cel::known_attribute_for;

        #[test]
        fn it_works() {
            let map = AttributeMap::new(
                [
                    known_attribute_for(&"request.method".into()).unwrap(),
                    known_attribute_for(&"request.referer".into()).unwrap(),
                    known_attribute_for(&"source.address".into()).unwrap(),
                    known_attribute_for(&"destination.port".into()).unwrap(),
                ]
                .into(),
            );

            println!("{:#?}", map.data);

            assert_eq!(3, map.data.len());
            assert!(map.data.contains_key("source"));
            assert!(map.data.contains_key("destination"));
            assert!(map.data.contains_key("request"));

            match map.data.get("source").unwrap() {
                Token::Node(map) => {
                    assert_eq!(map.len(), 1);
                    match map.get("address").unwrap() {
                        Token::Node(_) => panic!("Not supposed to get here!"),
                        Token::Value(v) => assert_eq!(v.path, "source.address".into()),
                    }
                }
                Token::Value(_) => panic!("Not supposed to get here!"),
            }

            match map.data.get("destination").unwrap() {
                Token::Node(map) => {
                    assert_eq!(map.len(), 1);
                    match map.get("port").unwrap() {
                        Token::Node(_) => panic!("Not supposed to get here!"),
                        Token::Value(v) => assert_eq!(v.path, "destination.port".into()),
                    }
                }
                Token::Value(_) => panic!("Not supposed to get here!"),
            }

            match map.data.get("request").unwrap() {
                Token::Node(map) => {
                    assert_eq!(map.len(), 2);
                    assert!(map.get("method").is_some());
                    match map.get("method").unwrap() {
                        Token::Node(_) => panic!("Not supposed to get here!"),
                        Token::Value(v) => assert_eq!(v.path, "request.method".into()),
                    }
                    assert!(map.get("referer").is_some());
                    match map.get("referer").unwrap() {
                        Token::Node(_) => panic!("Not supposed to get here!"),
                        Token::Value(v) => assert_eq!(v.path, "request.referer".into()),
                    }
                }
                Token::Value(_) => panic!("Not supposed to get here!"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::data::cel::{known_attribute_for, Expression, Predicate};
    use crate::data::property;
    use cel_interpreter::objects::ValueType;

    #[test]
    fn predicates() {
        let predicate = Predicate::new("source.port == 65432").expect("This is valid CEL!");
        property::test::TEST_PROPERTY_VALUE
            .set(Some(("source.port".into(), 65432_i64.to_le_bytes().into())));
        assert!(predicate.test());
    }

    #[test]
    fn expressions_sort_properties() {
        let value = Expression::new(
            "auth.identity.anonymous && auth.identity != null && auth.identity.foo > 3",
        )
        .unwrap();
        assert_eq!(value.attributes.len(), 3);
        assert_eq!(value.attributes[0].path, "auth.identity".into());
    }

    #[test]
    fn expressions_to_json_resolve() {
        property::test::TEST_PROPERTY_VALUE.set(Some((
            property::Path::new(vec![
                "filter_state",
                "wasm.kuadrant.auth.identity.anonymous",
            ]),
            "true".bytes().collect(),
        )));
        let value = Expression::new("auth.identity.anonymous").unwrap().eval();
        assert_eq!(value, true.into());

        property::test::TEST_PROPERTY_VALUE.set(Some((
            property::Path::new(vec!["filter_state", "wasm.kuadrant.auth.identity.age"]),
            "42".bytes().collect(),
        )));
        let value = Expression::new("auth.identity.age").unwrap().eval();
        assert_eq!(value, 42.into());

        property::test::TEST_PROPERTY_VALUE.set(Some((
            property::Path::new(vec!["filter_state", "wasm.kuadrant.auth.identity.age"]),
            "42.3".bytes().collect(),
        )));
        let value = Expression::new("auth.identity.age").unwrap().eval();
        assert_eq!(value, 42.3.into());

        property::test::TEST_PROPERTY_VALUE.set(Some((
            property::Path::new(vec!["filter_state", "wasm.kuadrant.auth.identity.age"]),
            "\"John\"".bytes().collect(),
        )));
        let value = Expression::new("auth.identity.age").unwrap().eval();
        assert_eq!(value, "John".into());

        property::test::TEST_PROPERTY_VALUE.set(Some((
            property::Path::new(vec!["filter_state", "wasm.kuadrant.auth.identity.name"]),
            "-42".bytes().collect(),
        )));
        let value = Expression::new("auth.identity.name").unwrap().eval();
        assert_eq!(value, (-42).into());

        // let's fall back to strings, as that's what we read and set in store_metadata
        property::test::TEST_PROPERTY_VALUE.set(Some((
            property::Path::new(vec!["filter_state", "wasm.kuadrant.auth.identity.age"]),
            "some random crap".bytes().collect(),
        )));
        let value = Expression::new("auth.identity.age").unwrap().eval();
        assert_eq!(value, "some random crap".into());
    }

    #[test]
    fn attribute_resolve() {
        property::test::TEST_PROPERTY_VALUE.set(Some((
            "destination.port".into(),
            80_i64.to_le_bytes().into(),
        )));
        let value = known_attribute_for(&"destination.port".into())
            .unwrap()
            .get();
        assert_eq!(value, 80.into());
        property::test::TEST_PROPERTY_VALUE
            .set(Some(("request.method".into(), "GET".bytes().collect())));
        let value = known_attribute_for(&"request.method".into()).unwrap().get();
        assert_eq!(value, "GET".into());
    }

    #[test]
    fn finds_known_attributes() {
        let path = "request.method".into();
        let attr = known_attribute_for(&path).expect("Must be a hit!");
        assert_eq!(attr.path, path);
        match attr.cel_type {
            Some(ValueType::String) => {}
            _ => panic!("Not supposed to get here!"),
        }
    }
}
