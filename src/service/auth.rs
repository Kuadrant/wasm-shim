use crate::attribute::get_attribute;
use crate::envoy::{
    Address, AttributeContext, AttributeContext_HttpRequest, AttributeContext_Peer,
    AttributeContext_Request, CheckRequest, Metadata, SocketAddress,
};
use crate::filter::http_context::TracingHeader;
use crate::service::Service;
use chrono::{DateTime, FixedOffset, Timelike};
use protobuf::well_known_types::Timestamp;
use protobuf::Message;
use proxy_wasm::hostcalls;
use proxy_wasm::hostcalls::dispatch_grpc_call;
use proxy_wasm::types::{Bytes, MapType, Status};
use std::collections::HashMap;
use std::time::Duration;

const AUTH_SERVICE_NAME: &str = "envoy.service.auth.v3.Authorization";
const AUTH_METHOD_NAME: &str = "Check";

pub struct AuthService {
    endpoint: String,
    tracing_headers: Vec<(TracingHeader, Bytes)>,
}

impl AuthService {
    pub fn new(endpoint: &str, metadata: Vec<(TracingHeader, Bytes)>) -> Self {
        Self {
            endpoint: String::from(endpoint),
            tracing_headers: metadata,
        }
    }

    pub fn message() -> CheckRequest {
        AuthService::build_check_req()
    }

    fn build_check_req() -> CheckRequest {
        let mut auth_req = CheckRequest::default();
        let mut attr = AttributeContext::default();
        attr.set_request(AuthService::build_request());
        attr.set_destination(AuthService::build_peer(
            get_attribute::<String>("destination.address").unwrap_or_default(),
            get_attribute::<i64>("destination.port").unwrap_or_default() as u32,
        ));
        attr.set_source(AuthService::build_peer(
            get_attribute::<String>("source.address").unwrap_or_default(),
            get_attribute::<i64>("source.port").unwrap_or_default() as u32,
        ));
        // todo(adam-cattermole): for now we set the context_extensions to the request host
        // but this should take other info into account
        let context_extensions = HashMap::from([(
            "host".to_string(),
            attr.get_request().get_http().host.to_owned(),
        )]);
        attr.set_context_extensions(context_extensions);
        attr.set_metadata_context(Metadata::default());
        auth_req.set_attributes(attr);
        auth_req
    }

    fn build_request() -> AttributeContext_Request {
        let mut request = AttributeContext_Request::default();
        let mut http = AttributeContext_HttpRequest::default();
        let headers: HashMap<String, String> = hostcalls::get_map(MapType::HttpRequestHeaders)
            .unwrap()
            .into_iter()
            .collect();

        http.set_host(get_attribute::<String>("request.host").unwrap_or_default());
        http.set_method(get_attribute::<String>("request.method").unwrap_or_default());
        http.set_scheme(get_attribute::<String>("request.scheme").unwrap_or_default());
        http.set_path(get_attribute::<String>("request.path").unwrap_or_default());
        http.set_protocol(get_attribute::<String>("request.protocol").unwrap_or_default());

        http.set_headers(headers);
        request.set_time(get_attribute("request.time").map_or(
            Timestamp::new(),
            |date_time: DateTime<FixedOffset>| Timestamp {
                nanos: date_time.nanosecond() as i32,
                seconds: date_time.second() as i64,
                unknown_fields: Default::default(),
                cached_size: Default::default(),
            },
        ));
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

impl Service<CheckRequest> for AuthService {
    fn send(&self, message: CheckRequest) -> Result<u32, Status> {
        let msg = Message::write_to_bytes(&message).unwrap(); // TODO(adam-cattermole): Error Handling
        let metadata = self
            .tracing_headers
            .iter()
            .map(|(header, value)| (header.as_str(), value.as_slice()))
            .collect();

        dispatch_grpc_call(
            self.endpoint.as_str(),
            AUTH_SERVICE_NAME,
            AUTH_METHOD_NAME,
            metadata,
            Some(&msg),
            Duration::from_secs(5),
        )
    }
}