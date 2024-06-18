use crate::typing::TypedProperty;
pub use cel_interpreter::objects::ValueType as CelValueType;
use std::collections::BTreeMap;

type MapperFn = dyn Fn(Vec<u8>) -> TypedProperty;

pub struct EnvoyTypeMapper {
    pub known_properties: BTreeMap<String, (Box<MapperFn>, CelValueType)>,
}

impl EnvoyTypeMapper {
    pub fn new() -> Self {
        let mut properties: BTreeMap<String, (Box<MapperFn>, CelValueType)> = BTreeMap::new();
        properties.insert(
            "request.time".to_string(),
            (Box::new(TypedProperty::timestamp), CelValueType::Timestamp),
        );

        properties.insert(
            "request.id".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "request.protocol".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "request.scheme".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "request.host".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "request.method".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "request.path".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "request.url_path".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "request.query".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "request.referer".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "request.useragent".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "request.body".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "source.address".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "source.service".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "source.principal".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "source.certificate".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "destination.address".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "destination.service".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "destination.principal".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "destination.certificate".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "connection.requested_server_name".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "connection.tls_session.sni".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "connection.tls_version".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "connection.subject_local_certificate".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "connection.subject_peer_certificate".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "connection.dns_san_local_certificate".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "connection.dns_san_peer_certificate".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "connection.uri_san_local_certificate".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "connection.uri_san_peer_certificate".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "connection.sha256_peer_certificate_digest".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );
        properties.insert(
            "ratelimit.domain".to_string(),
            (Box::new(TypedProperty::string), CelValueType::String),
        );

        properties.insert(
            "request.size".to_string(),
            (Box::new(TypedProperty::integer), CelValueType::Int),
        );
        properties.insert(
            "source.port".to_string(),
            (Box::new(TypedProperty::integer), CelValueType::Int),
        );
        properties.insert(
            "destination.port".to_string(),
            (Box::new(TypedProperty::integer), CelValueType::Int),
        );
        properties.insert(
            "connection.id".to_string(),
            (Box::new(TypedProperty::integer), CelValueType::Int),
        );
        properties.insert(
            "ratelimit.hits_addend".to_string(),
            (Box::new(TypedProperty::integer), CelValueType::Int),
        );

        properties.insert(
            "request.headers".to_string(),
            (Box::new(TypedProperty::string_map), CelValueType::Map),
        );
        properties.insert(
            "request.context_extensions".to_string(),
            (Box::new(TypedProperty::string_map), CelValueType::Map),
        );
        properties.insert(
            "source.labels".to_string(),
            (Box::new(TypedProperty::string_map), CelValueType::Map),
        );
        properties.insert(
            "destination.labels".to_string(),
            (Box::new(TypedProperty::string_map), CelValueType::Map),
        );
        properties.insert(
            "filter_state".to_string(),
            (Box::new(TypedProperty::string_map), CelValueType::Map),
        );

        properties.insert(
            "connection.mtls".to_string(),
            (Box::new(TypedProperty::boolean), CelValueType::Bool),
        );

        properties.insert(
            "request.raw_body".to_string(),
            (Box::new(TypedProperty::bytes), CelValueType::Bytes),
        );
        properties.insert(
            "auth.identity".to_string(),
            (Box::new(TypedProperty::bytes), CelValueType::Bytes),
        );

        Self {
            known_properties: properties,
        }
    }

    pub fn typed(&self, path: &str, raw: Vec<u8>) -> Result<TypedProperty, Vec<u8>> {
        match self.known_properties.get(path) {
            None => Err(raw),
            Some(map) => Ok(map.0(raw)),
        }
    }

    pub fn cel_type(&self, path: &str) -> Option<&CelValueType> {
        match self.known_properties.get(path) {
            None => None,
            Some(map) => Some(&map.1),
        }
    }
}
