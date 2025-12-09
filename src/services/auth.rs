use std::collections::HashMap;
use std::ops::Deref;
use std::time::Duration;

use cel_interpreter::Value;
use chrono::{DateTime, FixedOffset};
use prost::Message;
use prost_types::{Struct, Timestamp};
use tracing::{debug, enabled, Level};

use super::{Service, ServiceError};
use crate::configuration::FailureMode;
use crate::data::attribute::AttributeError;
use crate::data::attribute::AttributeState;
use crate::data::Headers;
use crate::envoy::{
    address, attribute_context, socket_address, Address, AttributeContext, CheckRequest,
    CheckResponse, Metadata, SocketAddress,
};
use crate::kuadrant::ReqRespCtx;

const KUADRANT_METADATA_PREFIX: &str = "io.kuadrant";

pub struct AuthService {
    upstream_name: String,
    service_name: String,
    method: String,
    timeout: Duration,
    failure_mode: FailureMode,
}

impl Service for AuthService {
    type Response = CheckResponse;

    fn parse_message(&self, message: Vec<u8>) -> Result<Self::Response, ServiceError> {
        prost::Message::decode(&message[..])
            .map_err(|e| ServiceError::Decode(format!("CheckResponse: {e}")))
    }
}

impl AuthService {
    pub fn new(endpoint: String, timeout: Duration, failure_mode: FailureMode) -> Self {
        Self {
            upstream_name: endpoint,
            service_name: "envoy.service.auth.v3.Authorization".to_string(),
            method: "Check".to_string(),
            timeout,
            failure_mode,
        }
    }

    pub fn failure_mode(&self) -> FailureMode {
        self.failure_mode
    }

    pub fn dispatch_auth(&self, ctx: &mut ReqRespCtx, scope: &str) -> Result<u32, ServiceError> {
        let check_request = build_check_request(ctx, scope)
            .map_err(|e| ServiceError::Dispatch(format!("Failed to build CheckRequest: {e}")))?;
        let outgoing_message = check_request.encode_to_vec();

        self.dispatch(
            ctx,
            &self.upstream_name,
            &self.service_name,
            &self.method,
            outgoing_message,
            self.timeout,
        )
    }
}

fn build_check_request(ctx: &mut ReqRespCtx, scope: &str) -> Result<CheckRequest, AttributeError> {
    let request = build_request_context(ctx)?;

    let destination = build_peer_context(
        ctx.get_required::<String>("destination.address")?,
        ctx.get_required::<i64>("destination.port")? as u32,
    );
    let source = build_peer_context(
        ctx.get_required::<String>("source.address")?,
        ctx.get_required::<i64>("source.port")? as u32,
    );

    let context_extensions = HashMap::from([("host".to_string(), scope.to_string())]);
    let metadata = build_metadata(ctx)?;

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

fn build_metadata(ctx: &mut ReqRespCtx) -> Result<Metadata, AttributeError> {
    let mut metadata = Metadata::default();
    let request_data = ctx.eval_request_data();

    if request_data.is_empty() {
        return Ok(metadata);
    }

    let mut by_domain: HashMap<String, Vec<(String, prost_types::Value)>> = HashMap::new();
    for entry in request_data {
        let prost_value = match entry.result {
            Ok(AttributeState::Available(Value::Null)) | Ok(AttributeState::Pending) | Err(_) => {
                let cel_expr_field_value = prost_types::Value {
                    kind: Some(prost_types::value::Kind::StringValue(entry.source)),
                };
                let cel_expr_struct = Struct {
                    fields: [("cel_expr".to_string(), cel_expr_field_value)].into(),
                };
                prost_types::Value {
                    kind: Some(prost_types::value::Kind::StructValue(cel_expr_struct)),
                }
            }
            Ok(AttributeState::Available(value)) => match value {
                Value::Null => continue,
                Value::Int(i) => prost_types::Value {
                    kind: Some(prost_types::value::Kind::NumberValue(i as f64)),
                },
                Value::UInt(u) => prost_types::Value {
                    kind: Some(prost_types::value::Kind::NumberValue(u as f64)),
                },
                Value::Float(f) => prost_types::Value {
                    kind: Some(prost_types::value::Kind::NumberValue(f)),
                },
                Value::String(s) => prost_types::Value {
                    kind: Some(prost_types::value::Kind::StringValue(s.deref().clone())),
                },
                Value::Bool(b) => prost_types::Value {
                    kind: Some(prost_types::value::Kind::BoolValue(b)),
                },
                _ => continue,
            },
        };

        by_domain
            .entry(entry.domain)
            .or_default()
            .push((entry.field, prost_value));
    }

    for (domain, entries) in by_domain.into_iter() {
        let mut fields = Struct::default();

        for (field, prost_value) in entries {
            fields.fields.insert(field, prost_value);
        }

        let data_key = if domain.is_empty() {
            KUADRANT_METADATA_PREFIX.to_string()
        } else {
            format!("{KUADRANT_METADATA_PREFIX}.{domain}")
        };

        if enabled!(Level::DEBUG) {
            let mut field_names = fields.fields.keys().collect::<Vec<_>>();
            field_names.sort();
            debug!("Adding data: `{data_key}` with entries: {field_names:?}");
        }

        metadata.filter_metadata.insert(data_key, fields);
    }

    Ok(metadata)
}

fn build_request_context(
    ctx: &mut ReqRespCtx,
) -> Result<attribute_context::Request, AttributeError> {
    let headers = ctx.get_required::<Headers>("request.headers")?;
    let host = ctx.get_required::<String>("request.host")?;
    let method = ctx.get_required::<String>("request.method")?;
    let scheme = ctx.get_required::<String>("request.scheme")?;
    let path = ctx.get_required::<String>("request.path")?;
    let protocol = ctx.get_required::<String>("request.protocol")?;

    let date_time = ctx.get_required::<DateTime<FixedOffset>>("request.time")?;
    let time = Timestamp {
        nanos: date_time.timestamp_subsec_nanos() as i32,
        seconds: date_time.timestamp(),
    };

    Ok(attribute_context::Request {
        time: Some(time),
        http: Some(attribute_context::HttpRequest {
            host,
            method,
            scheme,
            path,
            protocol,
            headers: headers.into(),
            ..Default::default()
        }),
    })
}

fn build_peer_context(host: String, port: u32) -> attribute_context::Peer {
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
