use crate::data::{get_attribute, PropError, PropertyError};
use crate::envoy::{
    Address, AttributeContext, AttributeContext_HttpRequest, AttributeContext_Peer,
    AttributeContext_Request, CheckRequest, Metadata, SocketAddress,
};
use crate::service::errors::BuildMessageError;
use chrono::{DateTime, FixedOffset};
use protobuf::well_known_types::Timestamp;
use protobuf::Message;
use proxy_wasm::hostcalls;
use proxy_wasm::types::MapType;
use std::collections::HashMap;

pub const AUTH_SERVICE_NAME: &str = "envoy.service.auth.v3.Authorization";
pub const AUTH_METHOD_NAME: &str = "Check";

pub struct AuthService;

impl AuthService {
    pub fn request_message(ce_host: String) -> Result<CheckRequest, PropertyError> {
        AuthService::build_check_req(ce_host)
    }

    pub fn request_message_as_bytes(ce_host: String) -> Result<Vec<u8>, BuildMessageError> {
        Self::request_message(ce_host)
            .map_err(BuildMessageError::Property)?
            .write_to_bytes()
            .map_err(BuildMessageError::Serialization)
    }

    fn build_check_req(ce_host: String) -> Result<CheckRequest, PropertyError> {
        let mut auth_req = CheckRequest::default();
        let mut attr = AttributeContext::default();
        attr.set_request(AuthService::build_request()?);
        attr.set_destination(AuthService::build_peer(
            get_attribute::<String>(&"destination.address".into())?.unwrap_or_default(),
            get_attribute::<i64>(&"destination.port".into())?.unwrap_or_default() as u32,
        ));
        attr.set_source(AuthService::build_peer(
            get_attribute::<String>(&"source.address".into())?.unwrap_or_default(),
            get_attribute::<i64>(&"source.port".into())?.unwrap_or_default() as u32,
        ));
        // the ce_host is the identifier for authorino to determine which authconfig to use
        let context_extensions = HashMap::from([("host".to_string(), ce_host)]);
        attr.set_context_extensions(context_extensions);
        attr.set_metadata_context(Metadata::default());
        auth_req.set_attributes(attr);
        Ok(auth_req)
    }

    fn build_request() -> Result<AttributeContext_Request, PropertyError> {
        let mut request = AttributeContext_Request::default();
        let mut http = AttributeContext_HttpRequest::default();
        let headers: HashMap<String, String> = match hostcalls::get_map(MapType::HttpRequestHeaders)
        {
            Ok(header_map) => header_map.into_iter().collect(),
            Err(_) => {
                return Err(PropertyError::Get(PropError::new(
                    "Failed to retrieve headers".to_string(),
                )))
            }
        };

        http.set_host(get_attribute::<String>(&"request.host".into())?.ok_or(
            PropertyError::Get(PropError::new("request.host not set".to_string())),
        )?);
        http.set_method(get_attribute::<String>(&"request.method".into())?.ok_or(
            PropertyError::Get(PropError::new("request.method not set".to_string())),
        )?);
        http.set_scheme(get_attribute::<String>(&"request.scheme".into())?.ok_or(
            PropertyError::Get(PropError::new("request.scheme not set".to_string())),
        )?);
        http.set_path(get_attribute::<String>(&"request.path".into())?.ok_or(
            PropertyError::Get(PropError::new("request.path not set".to_string())),
        )?);
        http.set_protocol(get_attribute::<String>(&"request.protocol".into())?.ok_or(
            PropertyError::Get(PropError::new("request.protocol not set".to_string())),
        )?);

        http.set_headers(headers);
        request.set_time(
            get_attribute(&"request.time".into())?
                .map(|date_time: DateTime<FixedOffset>| Timestamp {
                    nanos: date_time.timestamp_subsec_nanos() as i32,
                    seconds: date_time.timestamp(),
                    unknown_fields: Default::default(),
                    cached_size: Default::default(),
                })
                .ok_or(PropertyError::Get(PropError::new(
                    "request.time not set".to_string(),
                )))?,
        );
        request.set_http(http);
        Ok(request)
    }

    fn build_peer(host: String, port: u32) -> AttributeContext_Peer {
        let mut peer = AttributeContext_Peer::default();
        let mut address = Address::default();
        let mut socket_address = SocketAddress::default();
        socket_address.set_address(host);
        socket_address.set_port_value(port);
        address.set_socket_address(socket_address);
        peer.set_address(address);
        peer
    }
}
