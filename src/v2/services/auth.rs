use crate::envoy::{
    address, attribute_context, socket_address, Address, AttributeContext, CheckRequest, Metadata,
    SocketAddress,
};
use crate::v2::data::attribute::{PropError, PropertyError};
use crate::v2::kuadrant::ReqRespCtx;
use crate::v2::kuadrant::Service;
use crate::v2::temp::GrpcRequest;
use chrono::{DateTime, FixedOffset};
use prost_types::Timestamp;
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

    fn dispatch(&self, _ctx: &mut ReqRespCtx, _scope: String) -> usize {
        // build message
        // let _msg = self.request_message(ctx);

        // send message

        todo!()
    }

    fn parse_message(&self, _message: Vec<u8>) -> Self::Response {
        todo!()
    }

    fn request_message(&self, _ctx: &mut ReqRespCtx, _scope: String) -> GrpcRequest {
        todo!()
    }
}

pub fn build_checkrequest(
    ctx: &mut ReqRespCtx,
    scope: String,
) -> Result<CheckRequest, PropertyError> {
    let request = build_request(ctx)?;
    let destination = build_peer(
        ctx.get_attribute::<String>("destination.address")?
            .unwrap_or_default(),
        ctx.get_attribute::<i64>("destination.port")?
            .unwrap_or_default() as u32,
    );
    let source = build_peer(
        ctx.get_attribute::<String>("source.address")?
            .unwrap_or_default(),
        ctx.get_attribute::<i64>("source.port")?.unwrap_or_default() as u32,
    );
    // the ce_host is the identifier for authorino to determine which authconfig to use
    let context_extensions = HashMap::from([("host".to_string(), scope)]);

    let mut metadata = Metadata::default();
    // handle request data

    Ok(CheckRequest {
        attributes: Some(AttributeContext {
            request: Some(request),
            destination: Some(destination),
            source: Some(source),
            context_extensions,
            metadata_context: Some(metadata),
        }),
    })
}

fn build_request(ctx: &ReqRespCtx) -> Result<attribute_context::Request, PropertyError> {
    let headers: HashMap<String, String> = ctx.get_attribute_map(&"request.headers".into())?;
    let host = ctx
        .get_attribute::<String>("request.host")?
        .ok_or(PropertyError::Get(PropError::new(
            "request.host not set".to_string(),
        )))?;
    let method = ctx
        .get_attribute::<String>("request.method")?
        .ok_or(PropertyError::Get(PropError::new(
            "request.method not set".to_string(),
        )))?;
    let scheme = ctx
        .get_attribute::<String>("request.scheme")?
        .ok_or(PropertyError::Get(PropError::new(
            "request.scheme not set".to_string(),
        )))?;
    let path = ctx
        .get_attribute::<String>("request.path")?
        .ok_or(PropertyError::Get(PropError::new(
            "request.path not set".to_string(),
        )))?;
    let protocol = ctx
        .get_attribute::<String>("request.protocol")?
        .ok_or(PropertyError::Get(PropError::new(
            "request.protocol not set".to_string(),
        )))?;

    let time = ctx
        .get_attribute("request.time")?
        .map(|date_time: DateTime<FixedOffset>| Timestamp {
            nanos: date_time.timestamp_subsec_nanos() as i32,
            seconds: date_time.timestamp(),
        })
        .ok_or(PropertyError::Get(PropError::new(
            "request.time not set".to_string(),
        )))?;

    Ok(attribute_context::Request {
        time: Some(time),
        http: Some(attribute_context::HttpRequest {
            host,
            method,
            scheme,
            path,
            protocol,
            headers,
            ..Default::default()
        }),
    })
}

pub(crate) fn build_peer(host: String, port: u32) -> attribute_context::Peer {
    attribute_context::Peer {
        address: Some(Address {
            address: Some(address::Address::SocketAddress(SocketAddress {
                address: host,
                port_specifier: Some(socket_address::PortSpecifier::PortValue(port)),
                ..Default::default()
            })),
        }),
        ..Default::default()
    }
}
