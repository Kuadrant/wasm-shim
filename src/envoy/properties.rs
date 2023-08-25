use crate::typing::TypedProperty;
use std::collections::BTreeMap;

type MapperFn = dyn Fn(Vec<u8>) -> TypedProperty;

pub struct EnvoyTypeMapper {
    known_properties: BTreeMap<String, Box<MapperFn>>,
}

impl EnvoyTypeMapper {
    pub fn new() -> Self {
        let mut properties: BTreeMap<String, Box<MapperFn>> = BTreeMap::new();
        properties.insert(
            "request.time".to_string(),
            Box::new(TypedProperty::timestamp),
        );

        properties.insert("request.id".to_string(), Box::new(TypedProperty::string));
        properties.insert(
            "request.protocol".to_string(),
            Box::new(TypedProperty::string),
        );
        properties.insert(
            "request.scheme".to_string(),
            Box::new(TypedProperty::string),
        );
        properties.insert("request.host".to_string(), Box::new(TypedProperty::string));
        properties.insert(
            "request.method".to_string(),
            Box::new(TypedProperty::string),
        );
        properties.insert("request.path".to_string(), Box::new(TypedProperty::string));
        properties.insert(
            "request.url_path".to_string(),
            Box::new(TypedProperty::string),
        );
        properties.insert("request.query".to_string(), Box::new(TypedProperty::string));
        properties.insert(
            "request.referer".to_string(),
            Box::new(TypedProperty::string),
        );
        properties.insert(
            "request.useragent".to_string(),
            Box::new(TypedProperty::string),
        );
        properties.insert("request.body".to_string(), Box::new(TypedProperty::string));
        properties.insert(
            "source.address".to_string(),
            Box::new(TypedProperty::string),
        );
        properties.insert(
            "source.service".to_string(),
            Box::new(TypedProperty::string),
        );
        properties.insert(
            "source.principal".to_string(),
            Box::new(TypedProperty::string),
        );
        properties.insert(
            "source.certificate".to_string(),
            Box::new(TypedProperty::string),
        );
        properties.insert(
            "destination.address".to_string(),
            Box::new(TypedProperty::string),
        );
        properties.insert(
            "destination.service".to_string(),
            Box::new(TypedProperty::string),
        );
        properties.insert(
            "destination.principal".to_string(),
            Box::new(TypedProperty::string),
        );
        properties.insert(
            "destination.certificate".to_string(),
            Box::new(TypedProperty::string),
        );
        properties.insert(
            "connection.requested_server_name".to_string(),
            Box::new(TypedProperty::string),
        );
        properties.insert(
            "connection.tls_session.sni".to_string(),
            Box::new(TypedProperty::string),
        );
        properties.insert(
            "connection.tls_version".to_string(),
            Box::new(TypedProperty::string),
        );
        properties.insert(
            "connection.subject_local_certificate".to_string(),
            Box::new(TypedProperty::string),
        );
        properties.insert(
            "connection.subject_peer_certificate".to_string(),
            Box::new(TypedProperty::string),
        );
        properties.insert(
            "connection.dns_san_local_certificate".to_string(),
            Box::new(TypedProperty::string),
        );
        properties.insert(
            "connection.dns_san_peer_certificate".to_string(),
            Box::new(TypedProperty::string),
        );
        properties.insert(
            "connection.uri_san_local_certificate".to_string(),
            Box::new(TypedProperty::string),
        );
        properties.insert(
            "connection.uri_san_peer_certificate".to_string(),
            Box::new(TypedProperty::string),
        );
        properties.insert(
            "connection.sha256_peer_certificate_digest".to_string(),
            Box::new(TypedProperty::string),
        );
        properties.insert(
            "ratelimit.domain".to_string(),
            Box::new(TypedProperty::string),
        );

        properties.insert("request.size".to_string(), Box::new(TypedProperty::integer));
        properties.insert("source.port".to_string(), Box::new(TypedProperty::integer));
        properties.insert(
            "destination.port".to_string(),
            Box::new(TypedProperty::integer),
        );
        properties.insert(
            "connection.id".to_string(),
            Box::new(TypedProperty::integer),
        );
        properties.insert(
            "ratelimit.hits_addend".to_string(),
            Box::new(TypedProperty::integer),
        );

        properties.insert("metadata".to_string(), Box::new(TypedProperty::metadata));

        properties.insert(
            "request.headers".to_string(),
            Box::new(TypedProperty::string_map),
        );
        properties.insert(
            "request.context_extensions".to_string(),
            Box::new(TypedProperty::string_map),
        );
        properties.insert(
            "source.labels".to_string(),
            Box::new(TypedProperty::string_map),
        );
        properties.insert(
            "destination.labels".to_string(),
            Box::new(TypedProperty::string_map),
        );
        properties.insert(
            "filter_state".to_string(),
            Box::new(TypedProperty::string_map),
        );

        properties.insert(
            "auth.metadata".to_string(),
            Box::new(TypedProperty::complex_map),
        );
        properties.insert(
            "auth.authorization".to_string(),
            Box::new(TypedProperty::complex_map),
        );
        properties.insert(
            "auth.response".to_string(),
            Box::new(TypedProperty::complex_map),
        );
        properties.insert(
            "auth.callbacks".to_string(),
            Box::new(TypedProperty::complex_map),
        );

        properties.insert(
            "connection.mtls".to_string(),
            Box::new(TypedProperty::boolean),
        );

        properties.insert(
            "request.raw_body".to_string(),
            Box::new(TypedProperty::bytes),
        );
        properties.insert("auth.identity".to_string(), Box::new(TypedProperty::bytes));

        Self {
            known_properties: properties,
        }
    }

    pub fn typed(&self, path: &str, raw: Vec<u8>) -> Result<TypedProperty, Vec<u8>> {
        match self.known_properties.get(path) {
            None => Err(raw),
            Some(mapper) => Ok(mapper(raw)),
        }
    }
}
