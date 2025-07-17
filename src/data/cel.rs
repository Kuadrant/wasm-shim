use crate::data::cel::errors::{CelError, EvaluationError};
use crate::data::property::{host_get_map, Path};
use crate::data::{get_attribute, PropertyError};
use cel_interpreter::extractors::{Arguments, This};
use cel_interpreter::objects::{Key, Map, ValueType};
use cel_interpreter::{Context, ExecutionError, FunctionContext, ResolveResult, Value};
use cel_parser::{parse, Expression as CelExpression, Member, ParseError};
use chrono::{DateTime, FixedOffset};
#[cfg(feature = "debug-host-behaviour")]
use log::debug;
use log::{error, warn};
use proxy_wasm::types::{Bytes, Status};
use serde_json::Result as JsonResult;
use serde_json::Value as JsonValue;
use std::borrow::Cow;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt::{Debug, Display, Formatter};
use std::sync::{Arc, OnceLock};
use urlencoding::decode;

pub(super) mod errors {
    use crate::data::{Expression, PropertyError};
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
        Property(PropertyError),
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

    impl From<PropertyError> for CelError {
        fn from(e: PropertyError) -> Self {
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

        if expression.references().has_function("requestBodyJSON") {
            attributes.push(Attribute {
                // Injects the request body into the CEL context for internal use by the
                // requestBodyJSON function. The body is intentionally not exposed as a
                // top-level variable and cannot be directly referenced in the CEL expression.
                path: "@kuadrant.request\\.body".into(),
                cel_type: Some(ValueType::String),
            })
        }

        if expression.references().has_function("responseBodyJSON") {
            attributes.push(Attribute {
                path: "response.body".into(),
                cel_type: Some(ValueType::String),
            })
        }

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

    /// Evaluates the given expression using the provided resolver.
    pub fn eval<T>(&self, resolver: &mut T) -> Result<Value, CelError>
    where
        T: AttributeResolver,
    {
        let mut ctx = create_context();
        if self.extended {
            Self::add_extended_capabilities(&mut ctx)
        }
        let Map { map } = self.build_data_map(resolver)?;

        ctx.add_function("getHostProperty", get_host_property);
        ctx.add_function("requestBodyJSON", request_body_json);
        ctx.add_function("responseBodyJSON", response_body_json);

        for binding in [
            "request",
            "metadata",
            "source",
            "destination",
            "auth",
            "@kuadrant",
        ] {
            ctx.add_variable_from_value(
                binding,
                map.get(&binding.into()).cloned().unwrap_or(Value::Null),
            );
        }
        Value::resolve(&self.expression, &ctx).map_err(|e| e.into())
    }

    /// Add support for `queryMap`, see [`decode_query_string`]
    fn add_extended_capabilities(ctx: &mut Context) {
        ctx.add_function("queryMap", decode_query_string);
    }

    fn build_data_map<T>(&self, resolver: &mut T) -> Result<Map, PropertyError>
    where
        T: AttributeResolver,
    {
        data::AttributeMap::new(self.attributes.clone(), resolver).into()
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

fn request_body_json(ftx: &FunctionContext, json_pointer: Arc<String>) -> ResolveResult {
    eval_body_json(BodyOwner::Request, ftx, json_pointer)
}

fn response_body_json(ftx: &FunctionContext, json_pointer: Arc<String>) -> ResolveResult {
    eval_body_json(BodyOwner::Response, ftx, json_pointer)
}

enum BodyRef {
    Request,
    Response,
}

impl BodyRef {
    pub fn as_str(&self) -> &'static str {
        match self {
            BodyRef::Request => "request.body",
            BodyRef::Response => "response.body",
        }
    }
}

fn eval_body_json(
    body_ref: BodyOwner,
    ftx: &FunctionContext,
    json_pointer: Arc<String>,
) -> ResolveResult {
    let body_ref = body_ref.as_str();
    match ftx.ptx.get_variable("@kuadrant")? {
        Value::Map(map) => match map.get(&body_ref.into()) {
            None => Err(ftx.error(
                "Not supposed to get here! processing request body when it is not available",
            )),
            Some(Value::String(s)) => {
                let json_value: JsonResult<JsonValue> = serde_json::from_str(s.as_str());
                match json_value {
                    Err(err) => {
                        Err(ftx.error(format!("failed to parse {body_ref} as JSON: {err}",)))
                    }
                    Ok(json_value) => match json_value.pointer(json_pointer.as_str()) {
                        Some(value) => Ok(json_value_to_cel(value.clone())),
                        None => Err(ftx.error(format!(
                            "JSON Pointer '{json_pointer}' not found in {body_ref}",
                        ))),
                    },
                }
            }
            Some(v) => Err(ftx.error(format!(
                "found {body_ref} of type {}, expected String",
                v.type_of()
            ))),
        },
        _ => Err(ftx.error("Not supposed to get here! @kuadrant internal variable is not a Map")),
    }
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
                    message: format!("Status: {err:?}"),
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

#[derive(Clone, Debug, PartialEq)]
pub struct Predicate {
    expression: Expression,
}

pub type PredicateResult = Result<bool, EvaluationError>;

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

    pub fn test<T>(&self, resolver: &mut T) -> PredicateResult
    where
        T: AttributeResolver,
    {
        match self.expression.eval(resolver) {
            Ok(value) => match value {
                Value::Bool(result) => Ok(result),
                _ => Err(EvaluationError::new(
                    self.expression.clone(),
                    format!("Expected boolean value, got {value:?}"),
                )),
            },
            Err(CelError::Property(PropertyError::RequestBodyNotAvailable)) => {
                // TODO: EvaluationError is not specific enough to distinguish between errors
                // consider returning a more specific error type
                Err(EvaluationError::new(
                    self.expression.clone(),
                    "RequestBodyNotAvailable".into(),
                ))
            }
            Err(err) => {
                error!("Failed to evaluate `{:?}`: {err}", self.expression);
                Err(EvaluationError::new(
                    self.expression.clone(),
                    err.to_string(),
                ))
            }
        }
    }
}

pub trait PredicateVec {
    fn apply<T>(&self, resolver: &mut T) -> PredicateResult
    where
        T: AttributeResolver;
}

impl PredicateVec for Vec<Predicate> {
    fn apply<T>(&self, resolver: &mut T) -> PredicateResult
    where
        T: AttributeResolver,
    {
        if self.is_empty() {
            return Ok(true);
        }
        for predicate in self.iter() {
            // if it does not apply or errors exit early
            if !predicate.test(resolver)? {
                return Ok(false);
            }
        }
        Ok(true)
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
    pub fn get(&self) -> Result<Value, PropertyError> {
        match &self.cel_type {
            Some(t) => match t {
                ValueType::String => Ok(get_attribute::<String>(&self.path)?
                    .map(|v| Value::String(v.into()))
                    .unwrap_or(Value::Null)),
                ValueType::Int => Ok(get_attribute::<i64>(&self.path)?
                    .map(Value::Int)
                    .unwrap_or(Value::Null)),
                ValueType::UInt => Ok(get_attribute::<u64>(&self.path)?
                    .map(Value::UInt)
                    .unwrap_or(Value::Null)),
                ValueType::Float => Ok(get_attribute::<f64>(&self.path)?
                    .map(Value::Float)
                    .unwrap_or(Value::Null)),
                ValueType::Bool => Ok(get_attribute::<bool>(&self.path)?
                    .map(Value::Bool)
                    .unwrap_or(Value::Null)),
                ValueType::Bytes => Ok(get_attribute::<Vec<u8>>(&self.path)?
                    .map(|v| Value::Bytes(v.into()))
                    .unwrap_or(Value::Null)),
                ValueType::Timestamp => Ok(get_attribute::<DateTime<FixedOffset>>(&self.path)?
                    .map(Value::Timestamp)
                    .unwrap_or(Value::Null)),
                ValueType::Map => Ok(host_get_map(&self.path)
                    .map(cel_interpreter::objects::Map::from)
                    .map(Value::Map)
                    .unwrap_or(Value::Null)),
                _ => todo!("Need support for `{t}`s!"),
            },
            None => match get_attribute::<String>(&self.path)? {
                None => Ok(Value::Null),
                Some(json) => Ok(json_to_cel(&json)),
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

fn json_value_to_cel(json_value: JsonValue) -> Value {
    match json_value {
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
    }
}

fn json_to_cel(json: &str) -> Value {
    let json_value: Result<JsonValue, _> = serde_json::from_str(json);
    match json_value {
        Ok(json) => json_value_to_cel(json),
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
    use crate::data::cel::{Attribute, AttributeResolver};
    use crate::data::PropertyError;
    use cel_interpreter::objects::{Key, Map};
    use cel_interpreter::Value;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[derive(Debug)]
    enum Token {
        Node(HashMap<String, Token>),
        Value(Attribute),
    }

    pub struct AttributeMap<'a, T: AttributeResolver> {
        data: HashMap<String, Token>,
        resolver: &'a mut T,
    }

    impl<'a, T> AttributeMap<'a, T>
    where
        T: AttributeResolver,
    {
        pub fn new(attributes: Vec<Attribute>, resolver: &'a mut T) -> Self {
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
            Self {
                data: root,
                resolver,
            }
        }
    }

    impl<'a, T> From<AttributeMap<'a, T>> for Result<Map, PropertyError>
    where
        T: AttributeResolver,
    {
        fn from(value: AttributeMap<'a, T>) -> Self {
            map_to_value(value.data, value.resolver)
        }
    }

    fn map_to_value<T>(map: HashMap<String, Token>, resolver: &mut T) -> Result<Map, PropertyError>
    where
        T: AttributeResolver,
    {
        let mut out: HashMap<Key, Value> = HashMap::default();
        for (key, value) in map {
            let k = key.into();
            let v = match value {
                Token::Value(v) => resolver.resolve(&v)?,
                Token::Node(map) => Value::Map(map_to_value(map, resolver)?),
            };
            out.insert(k, v);
        }
        Ok(Map { map: Arc::new(out) })
    }

    #[cfg(test)]
    mod tests {
        use crate::data::cel::data::{AttributeMap, Token};
        use crate::data::cel::known_attribute_for;
        use crate::data::cel::PathCache;

        #[test]
        fn it_works() {
            let mut resolver = PathCache::default();
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
                &mut resolver,
            );

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

pub type AttributeResolverResult = Result<Value, PropertyError>;

pub trait AttributeResolver {
    fn resolve(&mut self, attribute: &Attribute) -> AttributeResolverResult;
}

#[derive(Default)]
pub struct PathCache(std::collections::HashMap<Path, Value>);

impl PathCache {
    pub fn insert_path(&mut self, path: Path, value: Value) {
        self.0.insert(path, value);
    }
}

impl From<HashMap<Path, Value>> for PathCache {
    fn from(map: HashMap<Path, Value>) -> Self {
        PathCache(map)
    }
}

impl AttributeResolver for PathCache {
    fn resolve(&mut self, attribute: &Attribute) -> AttributeResolverResult {
        match self.0.entry(attribute.path.clone()) {
            Entry::Occupied(entry) => Ok(entry.get().clone()),
            Entry::Vacant(entry) => match *attribute.path.tokens() {
                ["@kuadrant", "request.body"] => {
                    // Getting here means evaluating CEL expression with requestBodyJSON() function
                    // at the request headers phase. Need to wait.
                    // signals that the request body is not available
                    Err(PropertyError::RequestBodyNotAvailable)
                }
                ["@kuadrant", "response.body"] => {
                    // Getting here means evaluating CEL expression with responseBodyJSON() function
                    // before the response body phase. Need to wait.
                    // signals that the response body is not available
                    Err(PropertyError::ResponseBodyNotAvailable)
                }
                _ => {
                    let value = attribute.get()?;
                    entry.insert(value.clone());
                    Ok(value)
                }
            },
        }
    }
}

pub trait AttributeOwner {
    fn request_attributes(&self) -> Vec<&Attribute>;
}

impl AttributeOwner for Expression {
    fn request_attributes(&self) -> Vec<&Attribute> {
        self.attributes
            .iter()
            .filter(|&a| a.path.is_request())
            .collect()
    }
}

impl AttributeOwner for Predicate {
    fn request_attributes(&self) -> Vec<&Attribute> {
        self.expression.request_attributes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::cel::{known_attribute_for, CelError, Expression, PathCache, Predicate};
    use crate::data::property::Path;
    use crate::data::{property, PropError};
    use cel_interpreter::objects::ValueType;
    use cel_interpreter::{ExecutionError, Value};
    use std::collections::HashMap;
    use std::sync::Arc;

    pub struct TestResolver(std::collections::HashMap<Path, Value>);

    impl From<HashMap<Path, Value>> for TestResolver {
        fn from(map: HashMap<Path, Value>) -> Self {
            TestResolver(map)
        }
    }

    impl AttributeResolver for TestResolver {
        fn resolve(&mut self, attribute: &Attribute) -> AttributeResolverResult {
            match self.0.get(&attribute.path) {
                None => Err(PropertyError::Get(PropError::new("NotFound".into()))),
                Some(val) => Ok(val.clone()),
            }
        }
    }

    fn build_resolver(elems: Vec<(Path, Value)>) -> TestResolver {
        let mut map: HashMap<Path, Value> = HashMap::default();
        for (path, value) in elems {
            map.insert(path, value);
        }

        map.into()
    }

    fn build_resolver_with_request_body(body: String) -> TestResolver {
        build_resolver(vec![("@kuadrant.request\\.body".into(), body.into())])
    }

    fn build_resolver_with_response_body(body: String) -> TestResolver {
        build_resolver(vec![("@kuadrant.response\\.body".into(), body.into())])
    }

    #[test]
    fn predicates() {
        let predicate = Predicate::new("source.port == 65432").expect("This is valid CEL!");
        let mut resolver = build_resolver(vec![("source.port".into(), 65432_i64.into())]);
        assert!(predicate
            .test(&mut resolver)
            .expect("This must evaluate properly!"));
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
    fn attribute_to_json_resolve() {
        property::test::TEST_PROPERTY_VALUE.set(Some((
            property::Path::new(vec![
                "filter_state",
                "wasm.kuadrant.auth.identity.anonymous",
            ]),
            "true".bytes().collect(),
        )));
        let value = Attribute {
            path: "auth.identity.anonymous".into(),
            cel_type: None,
        }
        .get()
        .expect("This must resolve!");
        assert_eq!(value, true.into());

        property::test::TEST_PROPERTY_VALUE.set(Some((
            property::Path::new(vec!["filter_state", "wasm.kuadrant.auth.identity.age"]),
            "42".bytes().collect(),
        )));
        let value = Attribute {
            path: "auth.identity.age".into(),
            cel_type: None,
        }
        .get()
        .expect("This must evaluate!");
        assert_eq!(value, 42.into());

        property::test::TEST_PROPERTY_VALUE.set(Some((
            property::Path::new(vec!["filter_state", "wasm.kuadrant.auth.identity.age"]),
            "42.3".bytes().collect(),
        )));
        let value = Attribute {
            path: "auth.identity.age".into(),
            cel_type: None,
        }
        .get()
        .expect("This must evaluate!");
        assert_eq!(value, 42.3.into());

        property::test::TEST_PROPERTY_VALUE.set(Some((
            property::Path::new(vec!["filter_state", "wasm.kuadrant.auth.identity.name"]),
            "\"John\"".bytes().collect(),
        )));
        let value = Attribute {
            path: "auth.identity.name".into(),
            cel_type: None,
        }
        .get()
        .expect("This must evaluate!");
        assert_eq!(value, "John".into());

        property::test::TEST_PROPERTY_VALUE.set(Some((
            property::Path::new(vec!["filter_state", "wasm.kuadrant.auth.identity.age"]),
            "-42".bytes().collect(),
        )));
        let value = Attribute {
            path: "auth.identity.age".into(),
            cel_type: None,
        }
        .get()
        .expect("This must evaluate!");
        assert_eq!(value, (-42).into());

        // let's fall back to strings, as that's what we read and set in store_metadata
        property::test::TEST_PROPERTY_VALUE.set(Some((
            property::Path::new(vec!["filter_state", "wasm.kuadrant.auth.identity.age"]),
            "some random crap".bytes().collect(),
        )));
        let value = Attribute {
            path: "auth.identity.age".into(),
            cel_type: None,
        }
        .get()
        .expect("This must evaluate!");
        assert_eq!(value, "some random crap".into());
    }

    #[test]
    fn attribute_to_map_resolve() {
        property::test::TEST_MAP_VALUE.set(Some((
            "request.headers".into(),
            HashMap::from([
                ("key_a".into(), "val_a".into()),
                ("key_b".into(), "val_b".into()),
            ]),
        )));
        let value = known_attribute_for(&"request.headers".into())
            .expect("This is valid attribute")
            .get()
            .expect("This must resolve!");
        assert_eq!(
            value,
            HashMap::<String, String>::from([
                ("key_a".into(), "val_a".into()),
                ("key_b".into(), "val_b".into()),
            ])
            .into()
        );
    }

    #[test]
    fn decodes_query_string() {
        let mut resolver =
            build_resolver(vec![("request.query".into(), "param1=%F0%9F%91%BE%20&param2=Exterminate%21&%F0%9F%91%BE=123&%F0%9F%91%BE=456&%F0%9F%91%BE".into())]);
        let predicate = Predicate::route_rule(
            "queryMap(request.query, true)['param1'] == '👾 ' && \
            queryMap(request.query, true)['param2'] == 'Exterminate!' && \
            queryMap(request.query, true)['👾'][0] == '123' && \
            queryMap(request.query, true)['👾'][1] == '456' && \
            queryMap(request.query, true)['👾'][2] == '' \
                        ",
        )
        .expect("This is valid!");
        assert!(predicate
            .test(&mut resolver)
            .expect("This must evaluate properly!"));

        let mut resolver =
            build_resolver(vec![("request.query".into(), "param1=%F0%9F%91%BE%20&param2=Exterminate%21&%F0%9F%91%BE=123&%F0%9F%91%BE=456&%F0%9F%91%BE".into())]);
        let predicate = Predicate::route_rule(
            "queryMap(request.query, false)['param2'] == 'Exterminate!' && \
            queryMap(request.query, false)['👾'] == '123' \
                        ",
        )
        .expect("This is valid!");
        assert!(predicate
            .test(&mut resolver)
            .expect("This must evaluate properly!"));

        let mut resolver = build_resolver(vec![("request.query".into(), "%F0%9F%91%BE".into())]);
        let predicate =
            Predicate::route_rule("queryMap(request.query) == {'👾': ''}").expect("This is valid!");
        assert!(predicate
            .test(&mut resolver)
            .expect("This must evaluate properly!"));

        let mut resolver =
            build_resolver(vec![("request.query".into(), "param1=%F0%9F%91%BE%20&param2=Exterminate%21&%F0%9F%91%BE=123&%F0%9F%91%BE=456&%F0%9F%91%BE".into())]);
        let predicate = Predicate::route_rule(
            "'👾' in queryMap(request.query) ? queryMap(request.query)['👾'] == '123' : false",
        )
        .expect("This is valid!");
        assert_eq!(predicate.test(&mut resolver), Ok(true));
    }

    #[test]
    fn kuadrant_generated_predicates() {
        let headerr_value_map: HashMap<String, String> =
            HashMap::from([("X-Auth".into(), "kuadrant".into())]);
        let mut resolver =
            build_resolver(vec![("request.headers".into(), headerr_value_map.into())]);
        let predicate =
            Predicate::route_rule("request.headers.exists(h, h.lowerAscii() == 'x-auth' && request.headers[h] == 'kuadrant')").expect("This is valid!");
        assert_eq!(predicate.test(&mut resolver), Ok(true));
    }

    #[test]
    fn attribute_resolve() {
        property::test::TEST_PROPERTY_VALUE.set(Some((
            "destination.port".into(),
            80_i64.to_le_bytes().into(),
        )));
        let value = known_attribute_for(&"destination.port".into())
            .expect("destination.port known attribute exists")
            .get()
            .expect("There is no property error!");
        assert_eq!(value, 80.into());
        property::test::TEST_PROPERTY_VALUE
            .set(Some(("request.method".into(), "GET".bytes().collect())));
        let value = known_attribute_for(&"request.method".into())
            .expect("request.method known attribute exists")
            .get()
            .expect("There is no property error!");
        assert_eq!(value, "GET".into());
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
    fn expression_access_host() {
        property::test::TEST_PROPERTY_VALUE.set(Some((
            property::Path::new(vec!["foo", "bar.baz"]),
            b"\xCA\xFE".to_vec(),
        )));
        let mut resolver = PathCache::default();
        let value = Expression::new("getHostProperty(['foo', 'bar.baz'])")
            .expect("This is valid CEL!")
            .eval(&mut resolver)
            .expect("This must evaluate!");
        assert_eq!(value, Value::Bytes(Arc::new(b"\xCA\xFE".to_vec())));
    }

    #[test]
    fn expression_attribute_does_not_resolve() {
        let err = Expression::new("unknown.attribute")
            .expect("This is valid CEL!")
            .eval(&mut build_resolver(vec![]))
            .expect_err("This must error! body not available!");
        assert!(matches!(err, CelError::Property(PropertyError::Get(_))));
        if let CelError::Property(PropertyError::Get(r)) = err {
            assert_eq!(r, PropError::new("NotFound".into()));
        } else {
            unreachable!("Not supposed to get here!");
        }
    }

    #[test]
    fn expression_request_body_json_only_string_as_arg() {
        let mut resolver = build_resolver_with_request_body("{}".into());
        let err = Expression::new("requestBodyJSON()")
            .expect("This is valid CEL!")
            .eval(&mut resolver)
            .expect_err("This must error! func arg is missing!");
        assert!(matches!(
            err,
            CelError::Resolve(ExecutionError::InvalidArgumentCount { .. })
        ));

        let mut resolver = build_resolver_with_request_body("{}".into());
        let err = Expression::new("requestBodyJSON(['a', 'b'])")
            .expect("This is valid CEL!")
            .eval(&mut resolver)
            .expect_err("This must error! arrays not allowed as func arg!");
        assert!(matches!(
            err,
            CelError::Resolve(ExecutionError::UnexpectedType { .. })
        ));

        let mut resolver = build_resolver_with_request_body("{}".into());
        let err = Expression::new("requestBodyJSON(123435)")
            .expect("This is valid CEL!")
            .eval(&mut resolver)
            .expect_err("This must error! nums not allowed as func arg!");
        assert!(matches!(
            err,
            CelError::Resolve(ExecutionError::UnexpectedType { .. })
        ));

        let mut resolver = build_resolver_with_request_body("{}".into());
        let err = Expression::new("requestBodyJSON(true)")
            .expect("This is valid CEL!")
            .eval(&mut resolver)
            .expect_err("This must error! bools not allowed as func arg!");
        assert!(matches!(
            err,
            CelError::Resolve(ExecutionError::UnexpectedType { .. })
        ));
    }

    #[test]
    fn expression_request_body_json_body_wrong_type() {
        let mut resolver = build_resolver(vec![("@kuadrant.request\\.body".into(), 1.into())]);
        let err = Expression::new("requestBodyJSON('/foo')")
            .expect("This is valid CEL!")
            .eval(&mut resolver)
            .expect_err("This must error! body has unexpected type!");
        assert!(matches!(
            err,
            CelError::Resolve(ExecutionError::FunctionError { .. })
        ));
        if let CelError::Resolve(ExecutionError::FunctionError {
            function: f,
            message: m,
        }) = err
        {
            assert_eq!(f, String::from("requestBodyJSON"));
            assert!(m.contains("found request body of type int, expected String"));
        } else {
            unreachable!("Not supposed to get here!");
        }
    }

    #[test]
    fn expression_request_body_json_when_body_is_not_json() {
        let mut resolver = build_resolver_with_request_body("some crab".into());
        let err = Expression::new("requestBodyJSON('/foo')")
            .expect("This is valid CEL!")
            .eval(&mut resolver)
            .expect_err("This must error! body has unexpected type!");
        assert!(matches!(
            err,
            CelError::Resolve(ExecutionError::FunctionError { .. })
        ));
        if let CelError::Resolve(ExecutionError::FunctionError {
            function: f,
            message: m,
        }) = err
        {
            assert_eq!(f, String::from("requestBodyJSON"));
            assert!(m.contains("failed to parse request body as JSON"));
        } else {
            unreachable!("Not supposed to get here!");
        }
    }

    #[test]
    fn expression_request_body_json_when_pointer_does_not_exist() {
        let mut resolver = build_resolver_with_request_body("{}".into());
        let err = Expression::new("requestBodyJSON('/foo')")
            .expect("This is valid CEL!")
            .eval(&mut resolver)
            .expect_err("This must error! body not available!");
        assert!(matches!(
            err,
            CelError::Resolve(ExecutionError::FunctionError { .. })
        ));
        if let CelError::Resolve(ExecutionError::FunctionError {
            function: f,
            message: m,
        }) = err
        {
            assert_eq!(f, String::from("requestBodyJSON"));
            assert_eq!(
                m,
                String::from("JSON Pointer '/foo' not found in request body")
            );
        } else {
            unreachable!("Not supposed to get here!");
        }
    }

    #[test]
    fn expression_request_body_json_body_exists() {
        let body = r#"
        {
            "name": "John Doe",
            "age": 43,
            "phones": [
                "+44 1234567",
                "+44 2345678"
            ]
        }"#;
        let mut resolver = build_resolver_with_request_body(body.into());
        let value = Expression::new("requestBodyJSON('/name')")
            .expect("This is valid CEL!")
            .eval(&mut resolver)
            .expect("This must evaluate!");
        assert_eq!(value, "John Doe".into());

        let value = Expression::new("requestBodyJSON('/age')")
            .expect("This is valid CEL!")
            .eval(&mut resolver)
            .expect("This must evaluate!");
        assert_eq!(value, 43.into());

        let value = Expression::new("requestBodyJSON('/phones/0')")
            .expect("This is valid CEL!")
            .eval(&mut resolver)
            .expect("This must evaluate!");
        assert_eq!(value, "+44 1234567".into());
    }

    #[test]
    fn path_cache_request_body_not_found() {
        let request_body_attr = Attribute {
            path: "@kuadrant.request\\.body".into(),
            cel_type: Some(ValueType::String),
        };

        let mut resolver = PathCache::default();
        let err = resolver
            .resolve(&request_body_attr)
            .expect_err("This must error! body not available!");

        assert!(matches!(err, PropertyError::RequestBodyNotAvailable));
    }

    #[test]
    fn path_cache_request_body_found() {
        let mut path_cache: PathCache = HashMap::from([(
            "@kuadrant.request\\.body".into(),
            "This is the request body!".into(),
        )])
        .into();
        let request_body_attr = Attribute {
            path: "@kuadrant.request\\.body".into(),
            cel_type: Some(ValueType::String),
        };

        let value = path_cache
            .resolve(&request_body_attr)
            .expect("This must resolve! body is available!");
        assert_eq!(value, "This is the request body!".into());
    }

    #[test]
    fn expression_request_attributes() {
        let expression = Expression::new("source.port == 65432").expect("This is valid CEL!");
        assert!(
            expression.request_attributes().is_empty(),
            "No request attributes expected!"
        );

        let expression =
            Expression::new("request.host && request.path == '/path'").expect("This is valid CEL!");
        assert_eq!(
            expression.request_attributes().len(),
            2,
            "Two requests attributes expected!"
        );

        let expression = Expression::new("requestBodyJSON('/foo')").expect("This is valid CEL!");
        assert!(
            expression.request_attributes().is_empty(),
            "No request attributes expected when requestBodyJSON func is used!"
        );
    }

    #[test]
    fn predicate_request_attributes() {
        let predicate = Predicate::new("source.port == 65432").expect("This is valid CEL!");
        assert!(
            predicate.request_attributes().is_empty(),
            "No request attributes expected!"
        );

        let predicate = Predicate::new("request.host== 'example.com'").expect("This is valid CEL!");
        assert_eq!(
            predicate.request_attributes().len(),
            1,
            "One request attributes expected!"
        );
    }

    #[test]
    fn expression_response_body_json_only_string_as_arg() {
        let mut resolver = build_resolver_with_response_body("{}".into());
        let err = Expression::new("responseBodyJSON()")
            .expect("This is valid CEL!")
            .eval(&mut resolver)
            .expect_err("This must error! func arg is missing!");
        assert!(matches!(
            err,
            CelError::Resolve(ExecutionError::InvalidArgumentCount { .. })
        ));

        let mut resolver = build_resolver_with_response_body("{}".into());
        let err = Expression::new("responseBodyJSON(['a', 'b'])")
            .expect("This is valid CEL!")
            .eval(&mut resolver)
            .expect_err("This must error! arrays not allowed as func arg!");
        assert!(matches!(
            err,
            CelError::Resolve(ExecutionError::UnexpectedType { .. })
        ));

        let mut resolver = build_resolver_with_response_body("{}".into());
        let err = Expression::new("responseBodyJSON(123435)")
            .expect("This is valid CEL!")
            .eval(&mut resolver)
            .expect_err("This must error! nums not allowed as func arg!");
        assert!(matches!(
            err,
            CelError::Resolve(ExecutionError::UnexpectedType { .. })
        ));

        let mut resolver = build_resolver_with_response_body("{}".into());
        let err = Expression::new("responseBodyJSON(true)")
            .expect("This is valid CEL!")
            .eval(&mut resolver)
            .expect_err("This must error! bools not allowed as func arg!");
        assert!(matches!(
            err,
            CelError::Resolve(ExecutionError::UnexpectedType { .. })
        ));
    }

    #[test]
    fn expression_response_body_json_body_wrong_type() {
        let mut resolver = build_resolver(vec![("response.body".into(), 1.into())]);
        let err = Expression::new("responseBodyJSON('/foo')")
            .expect("This is valid CEL!")
            .eval(&mut resolver)
            .expect_err("This must error! body has unexpected type!");
        assert!(matches!(
            err,
            CelError::Resolve(ExecutionError::FunctionError { .. })
        ));
        if let CelError::Resolve(ExecutionError::FunctionError {
            function: f,
            message: m,
        }) = err
        {
            assert_eq!(f, String::from("responseBodyJSON"));
            assert!(m.contains("found response body of type int, expected String"));
        } else {
            unreachable!("Not supposed to get here!");
        }
    }

    #[test]
    fn expression_response_body_json_when_body_is_not_json() {
        let mut resolver = build_resolver_with_response_body("some crab".into());
        let err = Expression::new("responseBodyJSON('/foo')")
            .expect("This is valid CEL!")
            .eval(&mut resolver)
            .expect_err("This must error! body has unexpected type!");
        assert!(matches!(
            err,
            CelError::Resolve(ExecutionError::FunctionError { .. })
        ));
        if let CelError::Resolve(ExecutionError::FunctionError {
            function: f,
            message: m,
        }) = err
        {
            assert_eq!(f, String::from("responseBodyJSON"));
            assert!(m.contains("failed to parse response body as JSON"));
        } else {
            unreachable!("Not supposed to get here!");
        }
    }

    #[test]
    fn expression_response_body_json_when_pointer_does_not_exist() {
        let mut resolver = build_resolver_with_response_body("{}".into());
        let err = Expression::new("responseBodyJSON('/foo')")
            .expect("This is valid CEL!")
            .eval(&mut resolver)
            .expect_err("This must error! body not available!");
        assert!(matches!(
            err,
            CelError::Resolve(ExecutionError::FunctionError { .. })
        ));
        if let CelError::Resolve(ExecutionError::FunctionError {
            function: f,
            message: m,
        }) = err
        {
            assert_eq!(f, String::from("responseBodyJSON"));
            assert_eq!(
                m,
                String::from("JSON Pointer '/foo' not found in response body")
            );
        } else {
            unreachable!("Not supposed to get here!");
        }
    }

    #[test]
    fn expression_response_body_json_body_exists() {
        let body = r#"
        {
            "name": "John Doe",
            "age": 43,
            "phones": [
                "+44 1234567",
                "+44 2345678"
            ]
        }"#;
        let mut resolver = build_resolver_with_response_body(body.into());
        let value = Expression::new("responseBodyJSON('/name')")
            .expect("This is valid CEL!")
            .eval(&mut resolver)
            .expect("This must evaluate!");
        assert_eq!(value, "John Doe".into());

        let value = Expression::new("responseBodyJSON('/age')")
            .expect("This is valid CEL!")
            .eval(&mut resolver)
            .expect("This must evaluate!");
        assert_eq!(value, 43.into());

        let value = Expression::new("responseBodyJSON('/phones/0')")
            .expect("This is valid CEL!")
            .eval(&mut resolver)
            .expect("This must evaluate!");
        assert_eq!(value, "+44 1234567".into());
    }

    #[test]
    fn path_cache_response_body_not_found() {
        let response_body_attr = Attribute {
            path: "response.body".into(),
            cel_type: Some(ValueType::String),
        };

        let mut resolver = PathCache::default();
        let err = resolver
            .resolve(&response_body_attr)
            .expect_err("This must error! body not available!");

        assert!(matches!(err, PropertyError::ResponseBodyNotAvailable));
    }
}
