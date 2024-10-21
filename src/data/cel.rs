use crate::data::get_attribute;
use crate::data::property::Path;
use cel_interpreter::objects::{Map, ValueType};
use cel_interpreter::{Context, Value};
use cel_parser::{parse, Expression as CelExpression, Member, ParseError};
use chrono::{DateTime, FixedOffset};
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::sync::OnceLock;

pub struct Expression {
    attributes: Vec<Attribute>,
    expression: CelExpression,
}

impl Expression {
    pub fn new(expression: &str) -> Result<Self, ParseError> {
        let expression = parse(expression)?;

        let mut props = Vec::with_capacity(5);
        properties(&expression, &mut props, &mut Vec::default());

        let attributes = props
            .into_iter()
            .map(|tokens| {
                know_attribute_for(&Path::new(tokens))
                    // resolve to known root, and then inspect proper location
                    // path = ["auth", "identity", "anonymous", ...]
                    // UnknownAttribute { known_root: Path, Path }
                    //
                    // e.g. known part: ["auth", "identity"] => map it proper location
                    // ...anonymous
                    .expect("Unknown attribute")
            })
            .collect();

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

impl Debug for Attribute {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Attribute {{ {:?} }}", self.path)
    }
}

impl Clone for Attribute {
    fn clone(&self) -> Self {
        Attribute {
            path: self.path.clone(),
            cel_type: copy(&self.cel_type),
        }
    }
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
        ("auth.identity.anonymous".into(), ValueType::Bytes),
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
    use crate::data::Attribute;
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
                            Token::Value(_) => unreachable!(), // that's a bit of a lie!
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
        use crate::data::know_attribute_for;

        #[test]
        fn it_works() {
            let map = AttributeMap::new(
                [
                    know_attribute_for(&"request.method".into()).unwrap(),
                    know_attribute_for(&"request.referer".into()).unwrap(),
                    know_attribute_for(&"source.address".into()).unwrap(),
                    know_attribute_for(&"destination.port".into()).unwrap(),
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
    use crate::data::know_attribute_for;
    use cel_interpreter::objects::ValueType;

    #[test]
    fn finds_known_attributes() {
        let path = "request.method".into();
        let attr = know_attribute_for(&path).expect("Must be a hit!");
        assert_eq!(attr.path, path);
        match attr.cel_type {
            ValueType::String => {}
            _ => panic!("Not supposed to get here!"),
        }
    }
}
