use crate::data::attribute::{AttributeError, AttributeState, Path};
use crate::data::cel::errors::{CelError, EvaluationError};
use crate::data::Headers;
use crate::v2::kuadrant::ReqRespCtx;
use cel_interpreter::extractors::{Arguments, This};
use cel_interpreter::objects::{Key, Map, ValueType};
use cel_interpreter::{Context, ExecutionError, ResolveResult, Value};
use cel_parser::{parse, Expression as CelExpression, Member, ParseError};
use chrono::{DateTime, FixedOffset};
#[cfg(feature = "debug-host-behaviour")]
use log::debug;
use log::warn;
use serde_json::Value as JsonValue;
use std::borrow::Cow;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt::{Debug, Display, Formatter};
use std::sync::{Arc, OnceLock};
use urlencoding::decode;

pub(crate) mod errors {
    use crate::data::attribute::AttributeError;
    use crate::data::Expression;
    use cel_interpreter::ExecutionError;
    use std::error::Error;
    use std::fmt::{Debug, Display, Formatter};

    #[derive(Debug)]
    pub struct EvaluationError {
        expression: Expression,
        message: String,
    }

    impl PartialEq for EvaluationError {
        fn eq(&self, other: &Self) -> bool {
            self.message == other.message
        }
    }

    #[derive(Debug, PartialEq)]
    pub enum CelError {
        Property(AttributeError),
        Resolve(ExecutionError),
    }

    impl Error for CelError {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            match self {
                CelError::Property(err) => Some(err),
                CelError::Resolve(err) => Some(err),
            }
        }
    }

    impl From<AttributeError> for CelError {
        fn from(e: AttributeError) -> Self {
            CelError::Property(e)
        }
    }

    impl From<ExecutionError> for CelError {
        fn from(e: ExecutionError) -> Self {
            CelError::Resolve(e)
        }
    }

    impl Display for CelError {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            match self {
                CelError::Property(e) => {
                    write!(f, "CelError::Property {{ {e:?} }}")
                }
                CelError::Resolve(e) => {
                    write!(f, "CelError::Resolve {{ {e:?} }}")
                }
            }
        }
    }

    impl EvaluationError {
        pub fn new(expression: Expression, message: String) -> EvaluationError {
            EvaluationError {
                expression,
                message,
            }
        }
    }

    impl Display for EvaluationError {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            write!(
                f,
                "EvaluationError {{ expression: {:?}, message: {} }}",
                self.expression, self.message
            )
        }
    }
}

#[derive(Clone, Debug)]
pub struct Expression {
    attributes: Vec<Attribute>,
    expression: CelExpression,
    extended: bool,
}

pub type EvalResult = Result<AttributeState<Value>, CelError>;

impl Display for Expression {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Expression {{ expression: {:?}, attributes: {:?}, extended: {} }}",
            self.expression, self.attributes, self.extended
        )
    }
}

impl PartialEq for Expression {
    fn eq(&self, other: &Self) -> bool {
        let attributes_match = self.attributes == other.attributes;
        let extended_match = self.extended == other.extended;
        let expressions_match =
            format!("{:?}", self.expression) == format!("{:?}", other.expression);
        attributes_match && extended_match && expressions_match
    }
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

    #[cfg(test)]
    pub fn new_extended(expression: &str) -> Result<Self, ParseError> {
        Self::new_expression(expression, true)
    }

    pub fn eval(&self, req_ctx: &ReqRespCtx) -> EvalResult {
        let mut ctx = create_context();
        if self.extended {
            Self::add_extended_capabilities(&mut ctx)
        }

        // Eagerly cache all attributes used by this expression
        let paths: Vec<Path> = self
            .attributes
            .iter()
            .map(|attr| attr.path.clone())
            .collect();
        req_ctx.ensure_attributes(&paths);

        let Map { map } = match self.build_data_map(req_ctx)? {
            AttributeState::Pending => return Ok(AttributeState::Pending),
            AttributeState::Available(m) => m,
        };

        for binding in ["request", "metadata", "source", "destination", "auth"] {
            ctx.add_variable_from_value(
                binding,
                map.get(&binding.into()).cloned().unwrap_or(Value::Null),
            );
        }

        let result = Value::resolve(&self.expression, &ctx).map_err(CelError::from)?;
        Ok(AttributeState::Available(result))
    }

    /// Add support for `queryMap`, see [`decode_query_string`]
    fn add_extended_capabilities(ctx: &mut Context) {
        ctx.add_function("queryMap", decode_query_string);
    }

    fn build_data_map(&self, req_ctx: &ReqRespCtx) -> Result<AttributeState<Map>, AttributeError> {
        data::AttributeMap::new(self.attributes.clone()).into(req_ctx)
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
            let key = decode(key)
                .unwrap_or_else(|e| {
                    warn!("failed to decode query key, using default: {e:?}");
                    Cow::from(key)
                })
                .into_owned();
            match map.entry(key.into()) {
                Entry::Occupied(mut e) => {
                    if allow_repeats {
                        if let Value::List(ref mut list) = e.get_mut() {
                            match Arc::get_mut(list) {
                                None =>  {
                                    return Err(ExecutionError::FunctionError {
                                        function: "decode_query_string".to_string(),
                                        message: "concurrent modifications not allowed! How is this even shared?".to_string(),
                                    })
                                }
                                Some(v) => v
                                    .push(new_v),
                            }
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

pub mod strings;

#[derive(Clone, Debug, PartialEq)]
pub struct Predicate {
    expression: Expression,
}

pub type PredicateResult = Result<AttributeState<bool>, EvaluationError>;

impl Predicate {
    pub fn new(predicate: &str) -> Result<Self, ParseError> {
        Ok(Self {
            expression: Expression::new(predicate)?,
        })
    }

    /// Unlike with [`Predicate::new`], a `Predicate::route_rule` is backed by an
    /// `Expression` that has extended capabilities enabled.
    /// See [`Expression::add_extended_capabilities`]
    #[cfg(test)]
    pub fn route_rule(predicate: &str) -> Result<Self, ParseError> {
        Ok(Self {
            expression: Expression::new_extended(predicate)?,
        })
    }

    pub fn test(&self, req_ctx: &ReqRespCtx) -> PredicateResult {
        match self.expression.eval(req_ctx) {
            Ok(AttributeState::Pending) => Ok(AttributeState::Pending),
            Ok(AttributeState::Available(value)) => match value {
                Value::Bool(result) => Ok(AttributeState::Available(result)),
                _ => Err(EvaluationError::new(
                    self.expression.clone(),
                    format!("Expected boolean value, got {value:?}"),
                )),
            },
            Err(err) => Err(EvaluationError::new(
                self.expression.clone(),
                err.to_string(),
            )),
        }
    }
}

pub trait PredicateVec {
    fn apply(&self, req_ctx: &ReqRespCtx) -> PredicateResult;
}

impl PredicateVec for Vec<Predicate> {
    fn apply(&self, req_ctx: &ReqRespCtx) -> PredicateResult {
        if self.is_empty() {
            return Ok(AttributeState::Available(true));
        }

        let paths: Vec<Path> = self
            .iter()
            .flat_map(|p| &p.expression.attributes)
            .map(|attr| attr.path.clone())
            .collect();
        req_ctx.ensure_attributes(&paths);

        for predicate in self.iter() {
            match predicate.test(req_ctx)? {
                AttributeState::Pending => {
                    return Ok(AttributeState::Pending);
                }
                AttributeState::Available(false) => {
                    return Ok(AttributeState::Available(false));
                }
                AttributeState::Available(true) => continue,
            }
        }

        Ok(AttributeState::Available(true))
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

impl PartialEq for Attribute {
    fn eq(&self, other: &Self) -> bool {
        let paths_match = self.path == other.path;
        let cel_types_match = match (&self.cel_type, &other.cel_type) {
            (Some(a), Some(b)) => a.to_string() == b.to_string(),
            (None, None) => true,
            _ => false,
        };

        paths_match && cel_types_match
    }
}

impl Attribute {
    pub fn get(&self, ctx: &ReqRespCtx) -> Result<AttributeState<Value>, AttributeError> {
        match &self.cel_type {
            Some(t) => match t {
                ValueType::String => Ok(ctx
                    .get_attribute_ref::<String>(&self.path)?
                    .map(|opt| opt.map(|s| Value::String(s.into())).unwrap_or(Value::Null))),
                ValueType::Int => Ok(ctx
                    .get_attribute_ref::<i64>(&self.path)?
                    .map(|opt| opt.map(Value::Int).unwrap_or(Value::Null))),
                ValueType::UInt => Ok(ctx
                    .get_attribute_ref::<u64>(&self.path)?
                    .map(|opt| opt.map(Value::UInt).unwrap_or(Value::Null))),
                ValueType::Float => Ok(ctx
                    .get_attribute_ref::<f64>(&self.path)?
                    .map(|opt| opt.map(Value::Float).unwrap_or(Value::Null))),
                ValueType::Bool => Ok(ctx
                    .get_attribute_ref::<bool>(&self.path)?
                    .map(|opt| opt.map(Value::Bool).unwrap_or(Value::Null))),
                ValueType::Bytes => Ok(ctx
                    .get_attribute_ref::<Vec<u8>>(&self.path)?
                    .map(|opt| opt.map(|v| Value::Bytes(v.into())).unwrap_or(Value::Null))),
                ValueType::Timestamp => Ok(ctx
                    .get_attribute_ref::<DateTime<FixedOffset>>(&self.path)?
                    .map(|opt| opt.map(Value::Timestamp).unwrap_or(Value::Null))),
                ValueType::Map => Ok(ctx.get_attribute_ref::<Headers>(&self.path)?.map(|opt| {
                    //todo(refactor/pull/245): We should think about and handle other types of maps
                    // other than Headers / Vec<(String, String)>
                    opt.map(|headers| {
                        let map: HashMap<String, String> = headers.into();
                        Value::Map(cel_interpreter::objects::Map::from(map))
                    })
                    .unwrap_or(Value::Null)
                })),
                _ => todo!("Need support for `{t}`s!"),
            },
            None => Ok(ctx
                .get_attribute_ref::<String>(&self.path)?
                .map(|opt| opt.map(|s| json_to_cel(&s)).unwrap_or(Value::Null))),
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
            #[allow(clippy::expect_used)]
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
        | CelExpression::Or(e1, e2)
        | CelExpression::And(e1, e2) => {
            properties(e1, all, path);
            properties(e2, all, path);
        }
        CelExpression::Ternary(e1, e2, e3) => {
            properties(e1, all, path);
            properties(e2, all, path);
            properties(e3, all, path);
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
            // The attributes of the values returned by functions are skipped.
            path.clear();
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
#[allow(dead_code)]
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
    use crate::data::attribute::{AttributeError, AttributeState};
    use crate::data::cel::Attribute;
    use crate::v2::kuadrant::ReqRespCtx;
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

    impl AttributeMap {
        pub fn into(self, req_ctx: &ReqRespCtx) -> Result<AttributeState<Map>, AttributeError> {
            map_to_value(self.data, req_ctx)
        }
    }

    fn map_to_value(
        map: HashMap<String, Token>,
        req_ctx: &ReqRespCtx,
    ) -> Result<AttributeState<Map>, AttributeError> {
        let mut out: HashMap<Key, Value> = HashMap::default();
        for (key, value) in map {
            let k = key.into();
            let v = match value {
                Token::Value(attr) => match attr.get(req_ctx)? {
                    AttributeState::Available(val) => val,
                    AttributeState::Pending => {
                        return Ok(AttributeState::Pending);
                    }
                },
                Token::Node(nested_map) => match map_to_value(nested_map, req_ctx)? {
                    AttributeState::Available(m) => Value::Map(m),
                    AttributeState::Pending => {
                        return Ok(AttributeState::Pending);
                    }
                },
            };
            out.insert(k, v);
        }

        Ok(AttributeState::Available(Map { map: Arc::new(out) }))
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
                        Token::Node(_) => unreachable!("Not supposed to get here!"),
                        Token::Value(v) => assert_eq!(v.path, "source.address".into()),
                    }
                }
                Token::Value(_) => unreachable!("Not supposed to get here!"),
            }

            match map.data.get("destination").expect("destination is some") {
                Token::Node(map) => {
                    assert_eq!(map.len(), 1);
                    match map.get("port").expect("port is some") {
                        Token::Node(_) => unreachable!("Not supposed to get here!"),
                        Token::Value(v) => assert_eq!(v.path, "destination.port".into()),
                    }
                }
                Token::Value(_) => unreachable!("Not supposed to get here!"),
            }

            match map.data.get("request").expect("request is some") {
                Token::Node(map) => {
                    assert_eq!(map.len(), 2);
                    assert!(map.get("method").is_some());
                    match map.get("method").expect("method is some") {
                        Token::Node(_) => unreachable!("Not supposed to get here!"),
                        Token::Value(v) => assert_eq!(v.path, "request.method".into()),
                    }
                    assert!(map.get("referer").is_some());
                    match map.get("referer").expect("referer is some") {
                        Token::Node(_) => unreachable!("Not supposed to get here!"),
                        Token::Value(v) => assert_eq!(v.path, "request.referer".into()),
                    }
                }
                Token::Value(_) => unreachable!("Not supposed to get here!"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::data::attribute::{AttributeState, Path};
    use crate::data::cel::{known_attribute_for, Expression, Predicate};
    use crate::v2::kuadrant::MockWasmHost;
    use crate::v2::kuadrant::ReqRespCtx;
    use cel_interpreter::objects::ValueType;
    use cel_interpreter::Value;
    use std::sync::Arc;

    #[test]
    fn predicates() {
        let mock_host = MockWasmHost::new()
            .with_property("source.port".into(), 65432_i64.to_le_bytes().to_vec());
        let ctx = ReqRespCtx::new(Arc::new(mock_host));
        let predicate = Predicate::new("source.port == 65432").expect("This is valid CEL!");
        assert_eq!(
            predicate.test(&ctx).expect("This must evaluate properly!"),
            AttributeState::Available(true)
        );
    }

    #[test]
    fn expressions_sort_properties() {
        let value = Expression::new(
            "auth.identity.anonymous && auth.identity != null && auth.identity.foo > 3",
        )
        .expect("This is valid CEL!");
        assert_eq!(value.attributes.len(), 3);
        assert_eq!(value.attributes[0].path, "auth.identity".into());

        let value = Expression::new("foo.bar && a.b.c").expect("This is valid CEL!");
        assert_eq!(value.attributes.len(), 2);
        assert_eq!(value.attributes[0].path, "foo.bar".into());
        assert_eq!(value.attributes[1].path, "a.b.c".into());

        let value = Expression::new("my_func(foo.bar).a.b > 3").expect("This is valid CEL!");
        assert_eq!(value.attributes.len(), 1);
        assert_eq!(value.attributes[0].path, "foo.bar".into());
    }

    #[test]
    fn expressions_to_json_resolve() {
        // Test boolean value
        let mock_host = MockWasmHost::new().with_property(
            Path::new(vec![
                "filter_state",
                "wasm.kuadrant.auth.identity.anonymous",
            ]),
            "true".bytes().collect(),
        );
        let ctx = ReqRespCtx::new(Arc::new(mock_host));
        let value = Expression::new("auth.identity.anonymous")
            .expect("This is valid CEL!")
            .eval(&ctx)
            .expect("This must evaluate!");
        assert_eq!(value, AttributeState::Available(true.into()));

        // Test integer value
        let mock_host = MockWasmHost::new().with_property(
            Path::new(vec!["filter_state", "wasm.kuadrant.auth.identity.age"]),
            "42".bytes().collect(),
        );
        let ctx = ReqRespCtx::new(Arc::new(mock_host));
        let value = Expression::new("auth.identity.age")
            .expect("This is valid CEL!")
            .eval(&ctx)
            .expect("This must evaluate!");
        assert_eq!(value, AttributeState::Available(Value::UInt(42)));

        // Test float value
        let mock_host = MockWasmHost::new().with_property(
            Path::new(vec!["filter_state", "wasm.kuadrant.auth.identity.age"]),
            "42.3".bytes().collect(),
        );
        let ctx = ReqRespCtx::new(Arc::new(mock_host));
        let value = Expression::new("auth.identity.age")
            .expect("This is valid CEL!")
            .eval(&ctx)
            .expect("This must evaluate!");
        assert_eq!(value, AttributeState::Available(Value::Float(42.3)));

        let mock_host = MockWasmHost::new().with_property(
            Path::new(vec!["filter_state", "wasm.kuadrant.auth.identity.age"]),
            "\"John\"".bytes().collect(),
        );
        let ctx = ReqRespCtx::new(Arc::new(mock_host));
        let value = Expression::new("auth.identity.age")
            .expect("This is valid CEL!")
            .eval(&ctx)
            .expect("This must evaluate!");
        assert_eq!(
            value,
            AttributeState::Available(Value::String("John".to_string().into()))
        );

        // Test negative integer
        let mock_host = MockWasmHost::new().with_property(
            Path::new(vec!["filter_state", "wasm.kuadrant.auth.identity.name"]),
            "-42".bytes().collect(),
        );
        let ctx = ReqRespCtx::new(Arc::new(mock_host));
        let value = Expression::new("auth.identity.name")
            .expect("This is valid CEL!")
            .eval(&ctx)
            .expect("This must evaluate!");
        assert_eq!(value, AttributeState::Available(Value::Int(-42)));

        // Test fallback to string for non-JSON
        let mock_host = MockWasmHost::new().with_property(
            Path::new(vec!["filter_state", "wasm.kuadrant.auth.identity.age"]),
            "some random crap".bytes().collect(),
        );
        let ctx = ReqRespCtx::new(Arc::new(mock_host));
        let value = Expression::new("auth.identity.age")
            .expect("This is valid CEL!")
            .eval(&ctx)
            .expect("This must evaluate!");
        assert_eq!(
            value,
            AttributeState::Available(Value::String("some random crap".to_string().into()))
        );
    }

    #[test]
    fn decodes_query_string() {
        let mock_host = MockWasmHost::new()
            .with_property("request.query".into(), "param1=%F0%9F%91%BE%20&param2=Exterminate%21&%F0%9F%91%BE=123&%F0%9F%91%BE=456&%F0%9F%91%BE".bytes().collect());
        let ctx = ReqRespCtx::new(Arc::new(mock_host));
        let predicate = Predicate::route_rule(
            "queryMap(request.query, true)['param1'] == '👾 ' && \
            queryMap(request.query, true)['param2'] == 'Exterminate!' && \
            queryMap(request.query, true)['👾'][0] == '123' && \
            queryMap(request.query, true)['👾'][1] == '456' && \
            queryMap(request.query, true)['👾'][2] == '' \
                        ",
        )
        .expect("This is valid!");
        assert_eq!(
            predicate.test(&ctx).expect("This must evaluate properly!"),
            AttributeState::Available(true)
        );

        let mock_host = MockWasmHost::new()
            .with_property("request.query".into(), "param1=%F0%9F%91%BE%20&param2=Exterminate%21&%F0%9F%91%BE=123&%F0%9F%91%BE=456&%F0%9F%91%BE".bytes().collect());
        let ctx = ReqRespCtx::new(Arc::new(mock_host));
        let predicate = Predicate::route_rule(
            "queryMap(request.query, false)['param2'] == 'Exterminate!' && \
            queryMap(request.query, false)['👾'] == '123' \
                        ",
        )
        .expect("This is valid!");
        assert_eq!(
            predicate.test(&ctx).expect("This must evaluate properly!"),
            AttributeState::Available(true)
        );

        let mock_host = MockWasmHost::new()
            .with_property("request.query".into(), "%F0%9F%91%BE".bytes().collect());
        let ctx = ReqRespCtx::new(Arc::new(mock_host));
        let predicate =
            Predicate::route_rule("queryMap(request.query) == {'👾': ''}").expect("This is valid!");
        assert_eq!(
            predicate.test(&ctx).expect("This must evaluate properly!"),
            AttributeState::Available(true)
        );
    }

    #[test]
    fn kuadrant_generated_predicates() {
        let mock_host = MockWasmHost::new()
            .with_property("request.query".into(), "param1=%F0%9F%91%BE%20&param2=Exterminate%21&%F0%9F%91%BE=123&%F0%9F%91%BE=456&%F0%9F%91%BE".bytes().collect());
        let ctx = ReqRespCtx::new(Arc::new(mock_host));
        let predicate = Predicate::route_rule(
            "'👾' in queryMap(request.query) ? queryMap(request.query)['👾'] == '123' : false",
        )
        .expect("This is valid!");
        assert_eq!(predicate.test(&ctx), Ok(AttributeState::Available(true)));

        let headers = vec![
            ("X-Auth".to_string(), "kuadrant".to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];
        let mock_host = MockWasmHost::new().with_map("request.headers".to_string(), headers);
        let ctx = ReqRespCtx::new(Arc::new(mock_host));
        let predicate =
            Predicate::route_rule("request.headers.exists(h, h.lowerAscii() == 'x-auth' && request.headers[h] == 'kuadrant')").expect("This is valid!");
        assert_eq!(predicate.test(&ctx), Ok(AttributeState::Available(true)));
    }

    #[test]
    fn attribute_resolve() {
        let mock_host = MockWasmHost::new()
            .with_property("destination.port".into(), 80_i64.to_le_bytes().to_vec());
        let ctx = ReqRespCtx::new(Arc::new(mock_host));
        let value = known_attribute_for(&"destination.port".into())
            .expect("destination.port known attribute exists")
            .get(&ctx)
            .expect("There is no property error!");
        assert_eq!(value, AttributeState::Available(80.into()));

        let mock_host =
            MockWasmHost::new().with_property("request.method".into(), "GET".bytes().collect());
        let ctx = ReqRespCtx::new(Arc::new(mock_host));
        let value = known_attribute_for(&"request.method".into())
            .expect("request.method known attribute exists")
            .get(&ctx)
            .expect("There is no property error!");
        assert_eq!(value, AttributeState::Available("GET".into()));
    }

    #[test]
    fn finds_known_attributes() {
        let path = "request.method".into();
        let attr = known_attribute_for(&path).expect("Must be a hit!");
        assert_eq!(attr.path, path);
        match attr.cel_type {
            Some(ValueType::String) => {}
            _ => unreachable!("Not supposed to get here!"),
        }
    }

    #[test]
    fn test_attribute_get_returns_pending() {
        let mock_host = MockWasmHost::new().with_pending_property("destination.port".into());
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let attr = known_attribute_for(&"destination.port".into())
            .expect("destination.port known attribute exists");
        let result = attr.get(&ctx).expect("No property error should occur");

        assert_eq!(result, AttributeState::Pending);
    }

    #[test]
    fn test_expression_eval_handles_pending_from_build_data_map() {
        let mock_host = MockWasmHost::new()
            .with_property("source.port".into(), 65432_i64.to_le_bytes().to_vec())
            .with_pending_property("destination.port".into());
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let expression = Expression::new("source.port == 65432 && destination.port == 80")
            .expect("This is valid CEL!");
        let result = expression.eval(&ctx).expect("Evaluation should succeed");

        assert_eq!(result, AttributeState::Pending);
    }

    #[test]
    fn test_predicate_test_returns_pending() {
        let mock_host = MockWasmHost::new().with_pending_property("source.port".into());
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let predicate = Predicate::new("source.port == 65432").expect("This is valid CEL!");
        let result = predicate.test(&ctx).expect("Test should succeed");

        assert_eq!(result, AttributeState::Pending);
    }

    #[test]
    fn test_predicate_vec_short_circuits_on_pending() {
        use crate::data::cel::PredicateVec;

        // First pred is Pending
        let mock_host = MockWasmHost::new()
            .with_pending_property("source.port".into())
            .with_property("destination.port".into(), 80_i64.to_le_bytes().to_vec());
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let predicates = vec![
            Predicate::new("source.port == 65432").expect("valid CEL"),
            Predicate::new("destination.port == 80").expect("valid CEL"),
        ];

        let result = predicates.apply(&ctx).expect("Apply should succeed");
        assert_eq!(result, AttributeState::Pending);

        // Second pred is Pending
        let mock_host = MockWasmHost::new()
            .with_property("source.port".into(), 65432_i64.to_le_bytes().to_vec())
            .with_pending_property("destination.port".into());
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let predicates = vec![
            Predicate::new("source.port == 65432").expect("valid CEL"),
            Predicate::new("destination.port == 80").expect("valid CEL"),
        ];

        let result = predicates.apply(&ctx).expect("Apply should succeed");
        assert_eq!(result, AttributeState::Pending);

        // First is false, second is Pending
        let mock_host = MockWasmHost::new()
            .with_property("source.port".into(), 12345_i64.to_le_bytes().to_vec())
            .with_pending_property("destination.port".into());
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let predicates = vec![
            Predicate::new("source.port == 65432").expect("valid CEL"),
            Predicate::new("destination.port == 80").expect("valid CEL"),
        ];

        let result = predicates.apply(&ctx).expect("Apply should succeed");
        assert_eq!(result, AttributeState::Available(false));

        // First is Pending, second is false
        let mock_host = MockWasmHost::new()
            .with_pending_property("source.port".into())
            .with_property("destination.port".into(), 443_i64.to_le_bytes().to_vec());
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let predicates = vec![
            Predicate::new("source.port == 65432").expect("valid CEL"),
            Predicate::new("destination.port == 80").expect("valid CEL"),
        ];

        let result = predicates.apply(&ctx).expect("Apply should succeed");
        assert_eq!(result, AttributeState::Pending);
    }

    #[test]
    fn test_attribute_none_converts_to_null() {
        let mock_host = MockWasmHost::new(); // No properties set
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let attr = known_attribute_for(&"request.method".into())
            .expect("request.method is a known attribute");
        let result = attr.get(&ctx).expect("Should not error");

        assert_eq!(result, AttributeState::Available(Value::Null));
    }

    #[test]
    fn test_map_to_value_returns_pending_for_nested_attribute() {
        use crate::data::cel::data::AttributeMap;

        let mock_host = MockWasmHost::new()
            .with_property("request.method".into(), "GET".bytes().collect())
            .with_pending_property("source.address".into())
            .with_property("destination.port".into(), 80_i64.to_le_bytes().to_vec());
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let attr_map = AttributeMap::new(vec![
            known_attribute_for(&"request.method".into())
                .expect("request.method known attribute exists"),
            known_attribute_for(&"source.address".into())
                .expect("source.address known attribute exists"),
            known_attribute_for(&"destination.port".into())
                .expect("destination.port known attribute exists"),
        ]);

        let result = attr_map
            .into(&ctx)
            .expect("Should not return AttributeError");
        assert_eq!(result, AttributeState::Pending);
    }

    #[test]
    fn test_expression_eval_with_complex_mixed_states() {
        // These tests evaluate the current behaviour but without eager data retrieval
        // they should all be evaluateable
        let mock_host = MockWasmHost::new()
            .with_property("source.port".into(), 65432_i64.to_le_bytes().to_vec())
            .with_pending_property("destination.port".into());
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let expression = Expression::new("source.port == 65432 || destination.port == 80")
            .expect("This is valid CEL!");
        let result = expression.eval(&ctx).expect("Evaluation should succeed");

        assert_eq!(
            result,
            AttributeState::Pending,
            "Expression with Pending attribute should return Pending"
        );

        // Nested expressions
        let mock_host = MockWasmHost::new()
            .with_property("source.port".into(), 65432_i64.to_le_bytes().to_vec())
            .with_pending_property("destination.port".into())
            .with_property("request.method".into(), "GET".bytes().collect());
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let expression = Expression::new(
            "(source.port == 65432 && destination.port == 80) || request.method == 'POST'",
        )
        .expect("This is valid CEL!");
        let result = expression.eval(&ctx).expect("Evaluation should succeed");
        assert_eq!(
            result,
            AttributeState::Pending,
            "Complex expression with Pending in subexpression should return Pending"
        );

        // Ternary with Pending condition
        let mock_host = MockWasmHost::new()
            .with_property("source.port".into(), 65432_i64.to_le_bytes().to_vec())
            .with_pending_property("destination.port".into());
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let expression = Expression::new("source.port == 65432 ? 443 : destination.port")
            .expect("This is valid CEL!");
        let result = expression.eval(&ctx).expect("Evaluation should succeed");

        assert_eq!(
            result,
            AttributeState::Pending,
            "Ternary expression with Pending condition should return Pending"
        );
    }
}
