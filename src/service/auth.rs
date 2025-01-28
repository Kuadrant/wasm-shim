use crate::data::get_attribute;
use crate::envoy::{
    Address, AttributeContext, AttributeContext_HttpRequest, AttributeContext_Peer,
    AttributeContext_Request, CheckRequest, Metadata, SocketAddress,
};
use chrono::{DateTime, FixedOffset};
use log::debug;
use protobuf::well_known_types::Timestamp;
use protobuf::Message;
use proxy_wasm::hostcalls;
use proxy_wasm::types::MapType;
use std::collections::HashMap;

pub const AUTH_SERVICE_NAME: &str = "envoy.service.auth.v3.Authorization";
pub const AUTH_METHOD_NAME: &str = "Check";

pub struct AuthService;

impl AuthService {
    pub fn request_message(ce_host: String) -> CheckRequest {
        AuthService::build_check_req(ce_host)
    }

    pub fn request_message_as_bytes(ce_host: String) -> Option<Vec<u8>> {
        Self::request_message(ce_host)
            .write_to_bytes()
            .map_err(|e| debug!("Failed to write protobuf message to bytes: {e:?}"))
            .ok()
    }

    fn build_check_req(ce_host: String) -> CheckRequest {
        let mut auth_req = CheckRequest::default();
        let mut attr = AttributeContext::default();
        attr.set_request(AuthService::build_request());
        attr.set_destination(AuthService::build_peer(
            get_attribute::<String>(&"destination.address".into())
                .expect("Error!")
                .unwrap_or_default(),
            get_attribute::<i64>(&"destination.port".into())
                .expect("Error!")
                .unwrap_or_default() as u32,
        ));
        attr.set_source(AuthService::build_peer(
            get_attribute::<String>(&"source.address".into())
                .expect("Error!")
                .unwrap_or_default(),
            get_attribute::<i64>(&"source.port".into())
                .expect("Error!")
                .unwrap_or_default() as u32,
        ));
        // the ce_host is the identifier for authorino to determine which authconfig to use
        let context_extensions = HashMap::from([("host".to_string(), ce_host)]);
        attr.set_context_extensions(context_extensions);
        attr.set_metadata_context(Metadata::default());
        auth_req.set_attributes(attr);
        auth_req
    }

    fn build_request() -> AttributeContext_Request {
        let mut request = AttributeContext_Request::default();
        let mut http = AttributeContext_HttpRequest::default();
        let headers: HashMap<String, String> = hostcalls::get_map(MapType::HttpRequestHeaders)
            .expect("failed to retrieve HttpRequestHeaders from host")
            .into_iter()
            .collect();

        http.set_host(
            get_attribute::<String>(&"request.host".into())
                .expect("Error!")
                .unwrap_or_default(),
        );
        http.set_method(
            get_attribute::<String>(&"request.method".into())
                .expect("Error!")
                .unwrap_or_default(),
        );
        http.set_scheme(
            get_attribute::<String>(&"request.scheme".into())
                .expect("Error!")
                .unwrap_or_default(),
        );
        http.set_path(
            get_attribute::<String>(&"request.path".into())
                .expect("Error!")
                .unwrap_or_default(),
        );
        http.set_protocol(
            get_attribute::<String>(&"request.protocol".into())
                .expect("Error!")
                .unwrap_or_default(),
        );

        http.set_headers(headers);
        request.set_time(
            get_attribute(&"request.time".into())
                .expect("Error!")
                .map_or(Timestamp::new(), |date_time: DateTime<FixedOffset>| {
                    Timestamp {
                        nanos: date_time.timestamp_subsec_nanos() as i32,
                        seconds: date_time.timestamp(),
                        unknown_fields: Default::default(),
                        cached_size: Default::default(),
                    }
                }),
        );
        request.set_http(http);
        request
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
