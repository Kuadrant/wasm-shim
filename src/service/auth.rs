use crate::data::{get_attribute, AttributeResolver, Expression};
use crate::envoy::{
    address, attribute_context, socket_address, Address, AttributeContext, CheckRequest,
    DeniedHttpResponse, Metadata, SocketAddress, StatusCode,
};
use crate::service::errors::BuildMessageError;
use crate::service::DirectResponse;
use crate::v2::data::attribute::AttributeError;
use cel_interpreter::Value;
use chrono::{DateTime, FixedOffset};
use log::{debug, log_enabled};
use prost::Message;
use prost_types::{Struct, Timestamp};
use proxy_wasm::hostcalls;
use proxy_wasm::types::MapType;
use std::collections::HashMap;
use std::ops::Deref;

pub const AUTH_SERVICE_NAME: &str = "envoy.service.auth.v3.Authorization";
pub const AUTH_METHOD_NAME: &str = "Check";

impl From<DeniedHttpResponse> for DirectResponse {
    fn from(resp: DeniedHttpResponse) -> Self {
        let status_code = resp
            .status
            .as_ref()
            .map(|s| s.code)
            .unwrap_or(StatusCode::Forbidden as i32);
        let response_headers = resp
            .headers
            .iter()
            .filter_map(|header| {
                header
                    .header
                    .as_ref()
                    .map(|hv| (hv.key.to_owned(), hv.value.to_owned()))
            })
            .collect();
        Self::new(status_code as u32, response_headers, resp.body)
    }
}

pub struct AuthService;

const KUADRANT_METADATA_PREFIX: &str = "io.kuadrant";

impl AuthService {
    pub fn request_message<T>(
        ce_host: String,
        request_data: &[((String, String), Expression)],
        resolver: &mut T,
    ) -> Result<CheckRequest, AttributeError>
    where
        T: AttributeResolver,
    {
        AuthService::build_check_req(ce_host, request_data, resolver)
    }

    pub fn request_message_as_bytes<T>(
        ce_host: String,
        request_data: &[((String, String), Expression)],
        resolver: &mut T,
    ) -> Result<Vec<u8>, BuildMessageError>
    where
        T: AttributeResolver,
    {
        Ok(Self::request_message(ce_host, request_data, resolver)
            .map_err(BuildMessageError::Property)?
            .encode_to_vec())
    }

    fn build_check_req<T>(
        ce_host: String,
        request_data: &[((String, String), Expression)],
        resolver: &mut T,
    ) -> Result<CheckRequest, AttributeError>
    where
        T: AttributeResolver,
    {
        let request = AuthService::build_request()?;
        let destination = AuthService::build_peer(
            get_attribute::<String>(&"destination.address".into())?.unwrap_or_default(),
            get_attribute::<i64>(&"destination.port".into())?.unwrap_or_default() as u32,
        );
        let source = AuthService::build_peer(
            get_attribute::<String>(&"source.address".into())?.unwrap_or_default(),
            get_attribute::<i64>(&"source.port".into())?.unwrap_or_default() as u32,
        );
        // the ce_host is the identifier for authorino to determine which authconfig to use
        let context_extensions = HashMap::from([("host".to_string(), ce_host)]);

        let mut metadata = Metadata::default();
        if !request_data.is_empty() {
            let mut by_domain = HashMap::new();
            for ((domain, field), exp) in request_data {
                by_domain
                    .entry(domain)
                    .or_insert_with(Vec::new)
                    .push((field.to_string(), exp));
            }

            for (domain, entries) in by_domain.into_iter() {
                let mut fields = Struct::default();
                entries
                    .into_iter()
                    .for_each(|(field, expr)| match expr.eval(resolver) {
                        Ok(Value::Null) | Err(_) => {
                            let cel_expr_field_value = prost_types::Value {
                                kind: Some(prost_types::value::Kind::StringValue(
                                    expr.source().to_string(),
                                )),
                            };
                            let cel_expr_struct = Struct {
                                fields: [("cel_expr".to_string(), cel_expr_field_value)].into(),
                            };
                            let value = prost_types::Value {
                                kind: Some(prost_types::value::Kind::StructValue(cel_expr_struct)),
                            };
                            fields.fields.insert(field.clone(), value);
                        }
                        Ok(value) => {
                            if let Some(kind) = match value {
                                Value::Int(i) => {
                                    Some(prost_types::value::Kind::NumberValue(i as f64))
                                }
                                Value::UInt(u) => {
                                    Some(prost_types::value::Kind::NumberValue(u as f64))
                                }
                                Value::Float(f) => Some(prost_types::value::Kind::NumberValue(f)),
                                Value::String(s) => {
                                    Some(prost_types::value::Kind::StringValue(s.deref().clone()))
                                }
                                Value::Bool(b) => Some(prost_types::value::Kind::BoolValue(b)),
                                _ => None,
                            } {
                                let f_value = prost_types::Value { kind: Some(kind) };
                                fields.fields.insert(field.clone(), f_value);
                            }
                        }
                    });
                let data_key = if domain.is_empty() {
                    KUADRANT_METADATA_PREFIX.to_string()
                } else {
                    format!("{KUADRANT_METADATA_PREFIX}.{domain}")
                };
                if log_enabled!(log::Level::Debug) {
                    let mut fields = fields.fields.keys().collect::<Vec<_>>();
                    fields.sort();
                    debug!("Adding data: `{data_key}` with entries: {fields:?}",);
                }
                metadata.filter_metadata.insert(data_key, fields);
            }
        }

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

    fn build_request() -> Result<attribute_context::Request, AttributeError> {
        let headers: HashMap<String, String> = match hostcalls::get_map(MapType::HttpRequestHeaders)
        {
            Ok(header_map) => header_map.into_iter().collect(),
            Err(_) => {
                return Err(AttributeError::Retrieval(
                    "Failed to retrieve headers".to_string(),
                ))
            }
        };

        let host = get_attribute::<String>(&"request.host".into())?.ok_or(
            AttributeError::Retrieval("request.host not set".to_string()),
        )?;
        let method = get_attribute::<String>(&"request.method".into())?.ok_or(
            AttributeError::Retrieval("request.method not set".to_string()),
        )?;
        let scheme = get_attribute::<String>(&"request.scheme".into())?.ok_or(
            AttributeError::Retrieval("request.scheme not set".to_string()),
        )?;
        let path = get_attribute::<String>(&"request.path".into())?.ok_or(
            AttributeError::Retrieval("request.path not set".to_string()),
        )?;
        let protocol = get_attribute::<String>(&"request.protocol".into())?.ok_or(
            AttributeError::Retrieval("request.protocol not set".to_string()),
        )?;

        let time = get_attribute(&"request.time".into())?
            .map(|date_time: DateTime<FixedOffset>| Timestamp {
                nanos: date_time.timestamp_subsec_nanos() as i32,
                seconds: date_time.timestamp(),
            })
            .ok_or(AttributeError::Retrieval(
                "request.time not set".to_string(),
            ))?;

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

    fn build_peer(host: String, port: u32) -> attribute_context::Peer {
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
}
