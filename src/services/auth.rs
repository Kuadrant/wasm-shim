use std::collections::HashMap;
use std::time::Duration;

use cel::Value;
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

fn cel_value_to_prost(value: cel::Value) -> prost_types::Value {
    match value {
        Value::Null => prost_types::Value {
            kind: Some(prost_types::value::Kind::NullValue(0)),
        },
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
            kind: Some(prost_types::value::Kind::StringValue(s.to_string())),
        },
        Value::Bool(b) => prost_types::Value {
            kind: Some(prost_types::value::Kind::BoolValue(b)),
        },
        Value::Map(map) => {
            let mut fields = Struct::default();
            for (key, value) in map.map.iter() {
                if let cel::objects::Key::String(k) = key {
                    fields
                        .fields
                        .insert(k.to_string(), cel_value_to_prost(value.clone()));
                }
            }
            prost_types::Value {
                kind: Some(prost_types::value::Kind::StructValue(fields)),
            }
        }
        Value::List(list) => {
            let values = list.iter().map(|v| cel_value_to_prost(v.clone())).collect();
            prost_types::Value {
                kind: Some(prost_types::value::Kind::ListValue(
                    prost_types::ListValue { values },
                )),
            }
        }
        _ => prost_types::Value {
            kind: Some(prost_types::value::Kind::NullValue(0)),
        },
    }
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
            Ok(AttributeState::Available(value)) => {
                if matches!(value, Value::Null) {
                    continue;
                }
                cel_value_to_prost(value)
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use cel::objects::Key;
    use std::collections::HashMap;

    #[test]
    fn test_cel_value_to_prost_null() {
        let cel_value = Value::Null;
        let prost_value = cel_value_to_prost(cel_value);

        assert_eq!(
            prost_value.kind,
            Some(prost_types::value::Kind::NullValue(0))
        );
    }

    #[test]
    fn test_cel_value_to_prost_int() {
        let cel_value = Value::Int(42);
        let prost_value = cel_value_to_prost(cel_value);

        assert_eq!(
            prost_value.kind,
            Some(prost_types::value::Kind::NumberValue(42.0))
        );
    }

    #[test]
    fn test_cel_value_to_prost_uint() {
        let cel_value = Value::UInt(100);
        let prost_value = cel_value_to_prost(cel_value);

        assert_eq!(
            prost_value.kind,
            Some(prost_types::value::Kind::NumberValue(100.0))
        );
    }

    #[test]
    fn test_cel_value_to_prost_float() {
        let cel_value = Value::Float(3.14);
        let prost_value = cel_value_to_prost(cel_value);

        assert_eq!(
            prost_value.kind,
            Some(prost_types::value::Kind::NumberValue(3.14))
        );
    }

    #[test]
    fn test_cel_value_to_prost_string() {
        let cel_value = Value::String("hello".to_string().into());
        let prost_value = cel_value_to_prost(cel_value);

        assert_eq!(
            prost_value.kind,
            Some(prost_types::value::Kind::StringValue("hello".to_string()))
        );
    }

    #[test]
    fn test_cel_value_to_prost_bool() {
        let cel_value = Value::Bool(true);
        let prost_value = cel_value_to_prost(cel_value);

        assert_eq!(
            prost_value.kind,
            Some(prost_types::value::Kind::BoolValue(true))
        );
    }

    #[test]
    fn test_cel_value_to_prost_list() {
        let cel_list = vec![
            Value::Int(1),
            Value::String("test".to_string().into()),
            Value::Bool(true),
        ];
        let cel_value = Value::List(cel_list.into());
        let prost_value = cel_value_to_prost(cel_value);

        match prost_value.kind {
            Some(prost_types::value::Kind::ListValue(list)) => {
                assert_eq!(list.values.len(), 3);
                assert_eq!(
                    list.values[0].kind,
                    Some(prost_types::value::Kind::NumberValue(1.0))
                );
                assert_eq!(
                    list.values[1].kind,
                    Some(prost_types::value::Kind::StringValue("test".to_string()))
                );
                assert_eq!(
                    list.values[2].kind,
                    Some(prost_types::value::Kind::BoolValue(true))
                );
            }
            _ => panic!("Expected ListValue"),
        }
    }

    #[test]
    fn test_cel_value_to_prost_map() {
        let mut cel_map: HashMap<Key, Value> = HashMap::new();
        cel_map.insert(
            Key::String("name".to_string().into()),
            Value::String("Alice".to_string().into()),
        );
        cel_map.insert(Key::String("age".to_string().into()), Value::Int(30));
        cel_map.insert(Key::String("active".to_string().into()), Value::Bool(true));

        let cel_value = Value::Map(cel_map.into());
        let prost_value = cel_value_to_prost(cel_value);

        match prost_value.kind {
            Some(prost_types::value::Kind::StructValue(s)) => {
                assert_eq!(s.fields.len(), 3);
                assert_eq!(
                    s.fields.get("name").unwrap().kind,
                    Some(prost_types::value::Kind::StringValue("Alice".to_string()))
                );
                assert_eq!(
                    s.fields.get("age").unwrap().kind,
                    Some(prost_types::value::Kind::NumberValue(30.0))
                );
                assert_eq!(
                    s.fields.get("active").unwrap().kind,
                    Some(prost_types::value::Kind::BoolValue(true))
                );
            }
            _ => panic!("Expected StructValue"),
        }
    }

    #[test]
    fn test_cel_value_to_prost_map_with_non_string_keys_skipped() {
        let mut cel_map: HashMap<Key, Value> = HashMap::new();
        cel_map.insert(
            Key::String("valid".to_string().into()),
            Value::String("value".to_string().into()),
        );
        cel_map.insert(Key::Int(42), Value::String("skipped".to_string().into()));

        let cel_value = Value::Map(cel_map.into());
        let prost_value = cel_value_to_prost(cel_value);

        match prost_value.kind {
            Some(prost_types::value::Kind::StructValue(s)) => {
                assert_eq!(s.fields.len(), 1);
                assert!(s.fields.contains_key("valid"));
                assert!(!s.fields.contains_key("42"));
            }
            _ => panic!("Expected StructValue"),
        }
    }

    #[test]
    fn test_cel_value_to_prost_nested_map_in_list() {
        let mut inner_map: HashMap<Key, Value> = HashMap::new();
        inner_map.insert(
            Key::String("nested_key".to_string().into()),
            Value::String("nested_value".to_string().into()),
        );

        let cel_list = vec![Value::Map(inner_map.into()), Value::Int(123)];
        let cel_value = Value::List(cel_list.into());
        let prost_value = cel_value_to_prost(cel_value);

        match prost_value.kind {
            Some(prost_types::value::Kind::ListValue(list)) => {
                assert_eq!(list.values.len(), 2);

                match &list.values[0].kind {
                    Some(prost_types::value::Kind::StructValue(s)) => {
                        assert_eq!(s.fields.len(), 1);
                        assert_eq!(
                            s.fields.get("nested_key").unwrap().kind,
                            Some(prost_types::value::Kind::StringValue(
                                "nested_value".to_string()
                            ))
                        );
                    }
                    _ => panic!("Expected StructValue in list"),
                }

                assert_eq!(
                    list.values[1].kind,
                    Some(prost_types::value::Kind::NumberValue(123.0))
                );
            }
            _ => panic!("Expected ListValue"),
        }
    }

    #[test]
    fn test_cel_value_to_prost_nested_list_in_map() {
        let inner_list = vec![
            Value::String("item1".to_string().into()),
            Value::String("item2".to_string().into()),
        ];

        let mut cel_map: HashMap<Key, Value> = HashMap::new();
        cel_map.insert(
            Key::String("items".to_string().into()),
            Value::List(inner_list.into()),
        );
        cel_map.insert(Key::String("count".to_string().into()), Value::Int(2));

        let cel_value = Value::Map(cel_map.into());
        let prost_value = cel_value_to_prost(cel_value);

        match prost_value.kind {
            Some(prost_types::value::Kind::StructValue(s)) => {
                assert_eq!(s.fields.len(), 2);

                match &s.fields.get("items").unwrap().kind {
                    Some(prost_types::value::Kind::ListValue(list)) => {
                        assert_eq!(list.values.len(), 2);
                        assert_eq!(
                            list.values[0].kind,
                            Some(prost_types::value::Kind::StringValue("item1".to_string()))
                        );
                        assert_eq!(
                            list.values[1].kind,
                            Some(prost_types::value::Kind::StringValue("item2".to_string()))
                        );
                    }
                    _ => panic!("Expected ListValue in map"),
                }

                assert_eq!(
                    s.fields.get("count").unwrap().kind,
                    Some(prost_types::value::Kind::NumberValue(2.0))
                );
            }
            _ => panic!("Expected StructValue"),
        }
    }

    #[test]
    fn test_cel_value_to_prost_deeply_nested_structure() {
        // Create: { "user": { "tags": ["admin", "active"], "id": 1 } }
        let tags = vec![
            Value::String("admin".to_string().into()),
            Value::String("active".to_string().into()),
        ];
        let mut user_map: HashMap<Key, Value> = HashMap::new();
        user_map.insert(
            Key::String("tags".to_string().into()),
            Value::List(tags.into()),
        );
        user_map.insert(Key::String("id".to_string().into()), Value::Int(1));

        let mut root_map: HashMap<Key, Value> = HashMap::new();
        root_map.insert(
            Key::String("user".to_string().into()),
            Value::Map(user_map.into()),
        );

        let cel_value = Value::Map(root_map.into());
        let prost_value = cel_value_to_prost(cel_value);

        match prost_value.kind {
            Some(prost_types::value::Kind::StructValue(root)) => {
                assert_eq!(root.fields.len(), 1);

                match &root.fields.get("user").unwrap().kind {
                    Some(prost_types::value::Kind::StructValue(user)) => {
                        assert_eq!(user.fields.len(), 2);

                        match &user.fields.get("tags").unwrap().kind {
                            Some(prost_types::value::Kind::ListValue(list)) => {
                                assert_eq!(list.values.len(), 2);
                                assert_eq!(
                                    list.values[0].kind,
                                    Some(prost_types::value::Kind::StringValue(
                                        "admin".to_string()
                                    ))
                                );
                                assert_eq!(
                                    list.values[1].kind,
                                    Some(prost_types::value::Kind::StringValue(
                                        "active".to_string()
                                    ))
                                );
                            }
                            _ => panic!("Expected ListValue for tags"),
                        }

                        assert_eq!(
                            user.fields.get("id").unwrap().kind,
                            Some(prost_types::value::Kind::NumberValue(1.0))
                        );
                    }
                    _ => panic!("Expected StructValue for user"),
                }
            }
            _ => panic!("Expected StructValue for root"),
        }
    }

    #[test]
    fn test_cel_value_to_prost_empty_list() {
        let cel_value = Value::List(vec![].into());
        let prost_value = cel_value_to_prost(cel_value);

        match prost_value.kind {
            Some(prost_types::value::Kind::ListValue(list)) => {
                assert_eq!(list.values.len(), 0);
            }
            _ => panic!("Expected ListValue"),
        }
    }

    #[test]
    fn test_cel_value_to_prost_empty_map() {
        let cel_map: HashMap<Key, Value> = HashMap::new();
        let cel_value = Value::Map(cel_map.into());
        let prost_value = cel_value_to_prost(cel_value);

        match prost_value.kind {
            Some(prost_types::value::Kind::StructValue(s)) => {
                assert_eq!(s.fields.len(), 0);
            }
            _ => panic!("Expected StructValue"),
        }
    }
}
