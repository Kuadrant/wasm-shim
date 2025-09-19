use crate::envoy::{
    Address, AttributeContext, AttributeContext_HttpRequest, AttributeContext_Peer,
    AttributeContext_Request, CheckRequest, Metadata, SocketAddress,
};
use crate::v2::data::attribute::{PropError, PropertyError};
use crate::v2::kuadrant::ReqRespCtx;
use crate::v2::kuadrant::Service;
use crate::v2::temp::GrpcRequest;
use chrono::{DateTime, FixedOffset};
use protobuf::well_known_types::Timestamp;
use std::collections::HashMap;
use std::time::Duration;

struct AuthService {
    upstream_name: String,
    service_name: String,
    method: String,
    timeout: Duration,
}

impl Service for AuthService {
    type Response = String;

    fn dispatch(&self, ctx: &mut ReqRespCtx, scope: String) -> usize {
        // build message
        let _msg = self.request_message(ctx, scope);

        // send message

        todo!()
    }

    fn parse_message(&self, message: Vec<u8>) -> Self::Response {
        todo!()
    }

    fn request_message(&self, ctx: &mut ReqRespCtx, scope: String) -> GrpcRequest {
        todo!()
    }
}

pub fn build_checkrequest(
    ctx: &mut ReqRespCtx,
    scope: String,
) -> Result<CheckRequest, PropertyError> {
    let mut auth_req = CheckRequest::default();
    let mut attr = AttributeContext::default();
    attr.set_request(build_request(ctx)?);
    attr.set_destination(build_peer(
        ctx.get_attribute::<String>("destination.address")?
            .unwrap_or_default(),
        ctx.get_attribute::<i64>("destination.port")?
            .unwrap_or_default() as u32,
    ));
    attr.set_source(build_peer(
        ctx.get_attribute::<String>("source.address")?
            .unwrap_or_default(),
        ctx.get_attribute::<i64>("source.port")?.unwrap_or_default() as u32,
    ));
    // the context_extensions host is the identifier for authorino to determine which authconfig to use
    let context_extensions = HashMap::from([("host".to_string(), scope)]);
    attr.set_context_extensions(context_extensions);
    let mut metadata = Metadata::default();
    //todo: implement logic to retrieve request data from ctx
    attr.set_metadata_context(metadata);
    auth_req.set_attributes(attr);
    Ok(auth_req)
}

pub(crate) fn build_request(
    ctx: &mut ReqRespCtx,
) -> Result<AttributeContext_Request, PropertyError> {
    let mut request = AttributeContext_Request::default();
    let mut http = AttributeContext_HttpRequest::default();
    let headers: HashMap<String, String> = ctx.get_attribute_map(&"request.headers".into())?;
    http.set_host(
        ctx.get_attribute::<String>("request.host")?
            .ok_or(PropertyError::Get(PropError::new(
                "request.host not set".to_string(),
            )))?,
    );
    http.set_method(
        ctx.get_attribute::<String>("request.method")?
            .ok_or(PropertyError::Get(PropError::new(
                "request.method not set".to_string(),
            )))?,
    );
    http.set_scheme(
        ctx.get_attribute::<String>("request.scheme")?
            .ok_or(PropertyError::Get(PropError::new(
                "request.scheme not set".to_string(),
            )))?,
    );
    http.set_path(
        ctx.get_attribute::<String>("request.path")?
            .ok_or(PropertyError::Get(PropError::new(
                "request.path not set".to_string(),
            )))?,
    );
    http.set_protocol(ctx.get_attribute::<String>("request.protocol")?.ok_or(
        PropertyError::Get(PropError::new("request.protocol not set".to_string())),
    )?);

    http.set_headers(headers);
    request.set_time(
        ctx.get_attribute("request.time")?
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

pub(crate) fn build_peer(host: String, port: u32) -> AttributeContext_Peer {
    let mut peer = AttributeContext_Peer::default();
    let mut address = Address::default();
    let mut socket_address = SocketAddress::default();
    socket_address.set_address(host);
    socket_address.set_port_value(port);
    address.set_socket_address(socket_address);
    peer.set_address(address);
    peer
}
