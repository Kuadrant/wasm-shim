use crate::data::get_attribute;
use crate::data::property::{host_get_map, Path};
use cel_interpreter::extractors::{Arguments, This};
use cel_interpreter::objects::{Key, Map, ValueType};
use cel_interpreter::{Context, ExecutionError, ResolveResult, Value};
use cel_parser::{parse, Expression as CelExpression, Member, ParseError};
use chrono::{DateTime, FixedOffset};
#[cfg(feature = "debug-host-behaviour")]
use log::debug;
use log::{error, warn};
use proxy_wasm::types::{Bytes, Status};
use serde_json::Value as JsonValue;
use std::borrow::Cow;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::sync::{Arc, OnceLock};
use urlencoding::decode;

#[derive(Clone, Debug)]
pub struct Expression {
    attributes: Vec<Attribute>,
    expression: CelExpression,
    extended: bool,
}

impl Expression {
    pub fn new_expression(expression: &str, extended: bool) -> Result<Self, ParseError> {
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
            extended,
        })
    }

    pub fn new(expression: &str) -> Result<Self, ParseError> {
        Self::new_expression(expression, false)
    }

    pub fn new_extended(expression: &str) -> Result<Self, ParseError> {
        Self::new_expression(expression, true)
    }

    pub fn eval(&self) -> Result<Value, String> {
        let mut ctx = create_context();
        if self.extended {
            Self::add_extended_capabilities(&mut ctx)
        }
        let Map { map } = self.build_data_map();

        ctx.add_function("getHostProperty", get_host_property);

        for binding in ["request", "metadata", "source", "destination", "auth"] {
            ctx.add_variable_from_value(
                binding,
                map.get(&binding.into()).cloned().unwrap_or(Value::Null),
            );
        }
        Value::resolve(&self.expression, &ctx).map_err(|err| format!("{err:?}"))
    }

    /// Add support for `queryMap`, see [`decode_query_string`]
    fn add_extended_capabilities(ctx: &mut Context) {
        ctx.add_function("queryMap", decode_query_string);
    }

    fn build_data_map(&self) -> Map {
        data::AttributeMap::new(self.attributes.clone()).into()
    }
}

/// Decodes the query string and returns a Map where the key is the parameter's name and
/// the value is either a [`Value::String`] or a [`Value::List`] if the parameter's name is repeated
/// and the second arg is not set to `false`.
/// see [`tests::decodes_query_string`]
fn decode_query_string(This(s): This<Arc<String>>, Arguments(args): Arguments) -> ResolveResult {
    let allow_repeats = if args.len() == 2 {
        match &args[1] {
            Value::Bool(b) => *b,
            _ => false,
        }
    } else {
        false
    };
    let mut map: HashMap<Key, Value> = HashMap::default();
    for part in s.split('&') {
        let mut kv = part.split('=');
        if let (Some(key), Some(value)) = (kv.next(), kv.next().or(Some(""))) {
            let new_v: Value = decode(value)
                .unwrap_or_else(|e| {
                    warn!("failed to decode query value, using default: {e:?}");
                    Cow::from(value)
                })
                .into_owned()
                .into();
            match map.entry(
                decode(key)
                    .unwrap_or_else(|e| {
                        warn!("failed to decode query key, using default: {e:?}");
                        Cow::from(key)
                    })
                    .into_owned()
                    .into(),
            ) {
                Entry::Occupied(mut e) => {
                    if allow_repeats {
                        if let Value::List(ref mut list) = e.get_mut() {
                            Arc::get_mut(list)
                                .expect("This isn't ever shared!")
                                .push(new_v);
                        } else {
                            let v = e.get().clone();
                            let list = Value::List([v, new_v].to_vec().into());
                            e.insert(list);
                        }
                    }
                }
                Entry::Vacant(e) => {
                    e.insert(
                        decode(value)
                            .unwrap_or_else(|e| {
                                warn!("failed to decode query value, using default: {e:?}");
                                Cow::from(value)
                            })
                            .into_owned()
                            .into(),
                    );
                }
            }
        }
    }
    Ok(map.into())
}

#[cfg(test)]
pub fn inner_host_get_property(path: Vec<&str>) -> Result<Option<Bytes>, Status> {
    super::property::host_get_property(&Path::new(path))
}

#[cfg(not(test))]
pub fn inner_host_get_property(path: Vec<&str>) -> Result<Option<Bytes>, Status> {
    proxy_wasm::hostcalls::get_property(path)
}

fn get_host_property(This(this): This<Value>) -> ResolveResult {
    match this {
        Value::List(ref items) => {
            let mut tokens = Vec::with_capacity(items.len());
            for item in items.iter() {
                match item {
                    Value::String(token) => tokens.push(token.as_str()),
                    _ => return Err(this.error_expected_type(ValueType::String)),
                }
            }

            match inner_host_get_property(tokens) {
                Ok(data) => match data {
                    None => Ok(Value::Null),
                    Some(bytes) => Ok(Value::Bytes(bytes.into())),
                },
                Err(err) => Err(ExecutionError::FunctionError {
                    function: "hostcalls::get_property".to_string(),
                    message: format!("Status: {:?}", err),
                }),
            }
        }
        _ => Err(this.error_expected_type(ValueType::List)),
    }
}

fn create_context<'a>() -> Context<'a> {
    let mut ctx = Context::default();
    ctx.add_function("charAt", strings::char_at);
    ctx.add_function("indexOf", strings::index_of);
    ctx.add_function("join", strings::join);
    ctx.add_function("lastIndexOf", strings::last_index_of);
    ctx.add_function("lowerAscii", strings::lower_ascii);
    ctx.add_function("upperAscii", strings::upper_ascii);
    ctx.add_function("trim", strings::trim);
    ctx.add_function("replace", strings::replace);
    ctx.add_function("split", strings::split);
    ctx.add_function("substring", strings::substring);
    ctx
}

mod strings;

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

    /// Unlike with [`Predicate::new`], a `Predicate::route_rule` is backed by an
    /// `Expression` that has extended capabilities enabled.
    /// See [`Expression::add_extended_capabilities`]
    pub fn route_rule(predicate: &str) -> Result<Self, ParseError> {
        Ok(Self {
            expression: Expression::new_extended(predicate)?,
        })
    }

    pub fn test(&self) -> Result<bool, String> {
        match self.expression.eval() {
            Ok(value) => match value {
                Value::Bool(result) => Ok(result),
                _ => Err(format!("Expected boolean value, got {value:?}")),
            },
            Err(err) => Err(err),
        }
    }
}

pub trait PredicateVec {
    fn apply(&self) -> bool;
}

impl PredicateVec for Vec<Predicate> {
    fn apply(&self) -> bool {
        self.is_empty()
            || self.iter().all(|predicate| match predicate.test() {
                Ok(b) => b,
                Err(err) => {
                    error!("Failed to evaluate {:?}: {}", predicate, err);
                    panic!("Err out of this!")
                }
            })
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
                    n.as_u64().expect("Unreachable: number must be u64").into()
                } else if n.is_i64() {
                    n.as_i64().expect("Unreachable: number must be i64").into()
                } else {
                    n.as_f64().expect("Unreachable: number must be f64").into()
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
        ("connection.id".into(), ValueType::UInt),
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

#[cfg(feature = "debug-host-behaviour")]
pub fn debug_all_well_known_attributes() {
    let attributes = new_well_known_attribute_map();
    attributes.iter().for_each(|(key, value_type)| {
        match proxy_wasm::hostcalls::get_property(key.tokens()) {
            Ok(opt_bytes) => match opt_bytes {
                None => debug!("{:#?}({}): None", key, value_type),
                Some(bytes) => debug!("{:#?}({}): {:?}", key, value_type, bytes),
            },
            Err(err) => {
                debug!("{:#?}({}): (err) {:?}", key, value_type, err)
            }
        }
    })
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
                    known_attribute_for(&"request.method".into())
                        .expect("request.method known attribute exists"),
                    known_attribute_for(&"request.referer".into())
                        .expect("request.referer known attribute exists"),
                    known_attribute_for(&"source.address".into())
                        .expect("source.address known attribute exists"),
                    known_attribute_for(&"destination.port".into())
                        .expect("destination.port known attribute exists"),
                ]
                .into(),
            );

            println!("{:#?}", map.data);

            assert_eq!(3, map.data.len());
            assert!(map.data.contains_key("source"));
            assert!(map.data.contains_key("destination"));
            assert!(map.data.contains_key("request"));

            match map.data.get("source").expect("source is some") {
                Token::Node(map) => {
                    assert_eq!(map.len(), 1);
                    match map.get("address").expect("address is some") {
                        Token::Node(_) => panic!("Not supposed to get here!"),
                        Token::Value(v) => assert_eq!(v.path, "source.address".into()),
                    }
                }
                Token::Value(_) => panic!("Not supposed to get here!"),
            }

            match map.data.get("destination").expect("destination is some") {
                Token::Node(map) => {
                    assert_eq!(map.len(), 1);
                    match map.get("port").expect("port is some") {
                        Token::Node(_) => panic!("Not supposed to get here!"),
                        Token::Value(v) => assert_eq!(v.path, "destination.port".into()),
                    }
                }
                Token::Value(_) => panic!("Not supposed to get here!"),
            }

            match map.data.get("request").expect("request is some") {
                Token::Node(map) => {
                    assert_eq!(map.len(), 2);
                    assert!(map.get("method").is_some());
                    match map.get("method").expect("method is some") {
                        Token::Node(_) => panic!("Not supposed to get here!"),
                        Token::Value(v) => assert_eq!(v.path, "request.method".into()),
                    }
                    assert!(map.get("referer").is_some());
                    match map.get("referer").expect("referer is some") {
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
    use cel_interpreter::Value;
    use std::sync::Arc;

    #[test]
    fn predicates() {
        let predicate = Predicate::new("source.port == 65432").expect("This is valid CEL!");
        property::test::TEST_PROPERTY_VALUE
            .set(Some(("source.port".into(), 65432_i64.to_le_bytes().into())));
        assert!(predicate.test().expect("This must evaluate properly!"));
    }

    #[test]
    fn expressions_sort_properties() {
        let value = Expression::new(
            "auth.identity.anonymous && auth.identity != null && auth.identity.foo > 3",
        )
        .expect("This is valid CEL!");
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
        let value = Expression::new("auth.identity.anonymous")
            .expect("This is valid CEL!")
            .eval()
            .expect("This must evaluate!");
        assert_eq!(value, true.into());

        property::test::TEST_PROPERTY_VALUE.set(Some((
            property::Path::new(vec!["filter_state", "wasm.kuadrant.auth.identity.age"]),
            "42".bytes().collect(),
        )));
        let value = Expression::new("auth.identity.age")
            .expect("This is valid CEL!")
            .eval()
            .expect("This must evaluate!");
        assert_eq!(value, 42.into());

        property::test::TEST_PROPERTY_VALUE.set(Some((
            property::Path::new(vec!["filter_state", "wasm.kuadrant.auth.identity.age"]),
            "42.3".bytes().collect(),
        )));
        let value = Expression::new("auth.identity.age")
            .expect("This is valid CEL!")
            .eval()
            .expect("This must evaluate!");
        assert_eq!(value, 42.3.into());

        property::test::TEST_PROPERTY_VALUE.set(Some((
            property::Path::new(vec!["filter_state", "wasm.kuadrant.auth.identity.age"]),
            "\"John\"".bytes().collect(),
        )));
        let value = Expression::new("auth.identity.age")
            .expect("This is valid CEL!")
            .eval()
            .expect("This must evaluate!");
        assert_eq!(value, "John".into());

        property::test::TEST_PROPERTY_VALUE.set(Some((
            property::Path::new(vec!["filter_state", "wasm.kuadrant.auth.identity.name"]),
            "-42".bytes().collect(),
        )));
        let value = Expression::new("auth.identity.name")
            .expect("This is valid CEL!")
            .eval()
            .expect("This must evaluate!");
        assert_eq!(value, (-42).into());

        // let's fall back to strings, as that's what we read and set in store_metadata
        property::test::TEST_PROPERTY_VALUE.set(Some((
            property::Path::new(vec!["filter_state", "wasm.kuadrant.auth.identity.age"]),
            "some random crap".bytes().collect(),
        )));
        let value = Expression::new("auth.identity.age")
            .expect("This is valid CEL!")
            .eval()
            .expect("This must evaluate!");
        assert_eq!(value, "some random crap".into());
    }

    #[test]
    fn decodes_query_string() {
        property::test::TEST_PROPERTY_VALUE.set(Some((
            "request.query".into(),
            "param1=%F0%9F%91%BE%20&param2=Exterminate%21&%F0%9F%91%BE=123&%F0%9F%91%BE=456&%F0%9F%91%BE"
                .bytes()
                .collect(),
        )));
        let predicate = Predicate::route_rule(
            "queryMap(request.query, true)['param1'] == '👾 ' && \
            queryMap(request.query, true)['param2'] == 'Exterminate!' && \
            queryMap(request.query, true)['👾'][0] == '123' && \
            queryMap(request.query, true)['👾'][1] == '456' && \
            queryMap(request.query, true)['👾'][2] == '' \
                        ",
        )
        .expect("This is valid!");
        assert!(predicate.test().expect("This must evaluate properly!"));

        property::test::TEST_PROPERTY_VALUE.set(Some((
            "request.query".into(),
            "param1=%F0%9F%91%BE%20&param2=Exterminate%21&%F0%9F%91%BE=123&%F0%9F%91%BE=456&%F0%9F%91%BE"
                .bytes()
                .collect(),
        )));
        let predicate = Predicate::route_rule(
            "queryMap(request.query, false)['param2'] == 'Exterminate!' && \
            queryMap(request.query, false)['👾'] == '123' \
                        ",
        )
        .expect("This is valid!");
        assert!(predicate.test().expect("This must evaluate properly!"));

        property::test::TEST_PROPERTY_VALUE.set(Some((
            "request.query".into(),
            "%F0%9F%91%BE".bytes().collect(),
        )));
        let predicate =
            Predicate::route_rule("queryMap(request.query) == {'👾': ''}").expect("This is valid!");
        assert!(predicate.test().expect("This must evaluate properly!"));
    }

    #[test]
    fn kuadrant_generated_predicates() {
        property::test::TEST_PROPERTY_VALUE.set(Some((
            "request.query".into(),
            "param1=%F0%9F%91%BE%20&param2=Exterminate%21&%F0%9F%91%BE=123&%F0%9F%91%BE=456&%F0%9F%91%BE"
                .bytes()
                .collect(),
        )));
        let predicate = Predicate::route_rule(
            "'👾' in queryMap(request.query) ? queryMap(request.query)['👾'] == '123' : false",
        )
        .expect("This is valid!");
        assert_eq!(predicate.test(), Ok(true));

        let predicate =
            Predicate::route_rule("request.headers.exists(h, h.lowerAscii() == 'x-auth' && request.headers[h] == 'kuadrant')").expect("This is valid!");
        assert_eq!(predicate.test(), Ok(true));
    }

    #[test]
    fn attribute_resolve() {
        property::test::TEST_PROPERTY_VALUE.set(Some((
            "destination.port".into(),
            80_i64.to_le_bytes().into(),
        )));
        let value = known_attribute_for(&"destination.port".into())
            .expect("destination.port known attribute exists")
            .get();
        assert_eq!(value, 80.into());
        property::test::TEST_PROPERTY_VALUE
            .set(Some(("request.method".into(), "GET".bytes().collect())));
        let value = known_attribute_for(&"request.method".into())
            .expect("request.method known attribute exists")
            .get();
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

    #[test]
    fn expression_access_host() {
        property::test::TEST_PROPERTY_VALUE.set(Some((
            property::Path::new(vec!["foo", "bar.baz"]),
            b"\xCA\xFE".to_vec(),
        )));
        let value = Expression::new("getHostProperty(['foo', 'bar.baz'])")
            .expect("This is valid CEL!")
            .eval()
            .expect("This must evaluate!");
        assert_eq!(value, Value::Bytes(Arc::new(b"\xCA\xFE".to_vec())));
    }
}
