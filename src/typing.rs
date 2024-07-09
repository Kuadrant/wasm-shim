use crate::configuration::{PatternExpression, WhenConditionOperator};
use cel_interpreter::objects::{ValueType as CelValueType, ValueType};
use cel_parser::{ArithmeticOp, Atom, Expression, ParseError, RelationOp};
use chrono::{DateTime, Utc};
use std::fmt::Write;
use std::ops::Add;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use cel_parser::Expression::Ident;

pub enum TypedProperty {
    String(String),
    Integer(i64),
    Timestamp(SystemTime),
    Bytes(Vec<u8>),
}

impl TypedProperty {
    pub fn string(bytes: Vec<u8>) -> Self {
        match String::from_utf8(bytes) {
            Ok(string) => TypedProperty::String(string),
            Err(err) => TypedProperty::bytes(err.into_bytes()),
        }
    }

    pub fn integer(bytes: Vec<u8>) -> Self {
        if bytes.len() == 8 {
            TypedProperty::Integer(i64::from_le_bytes(
                bytes[..8].try_into().expect("This has to be 8 bytes long!"),
            ))
        } else {
            TypedProperty::bytes(bytes)
        }
    }

    pub fn timestamp(bytes: Vec<u8>) -> Self {
        if bytes.len() == 8 {
            TypedProperty::Timestamp(SystemTime::UNIX_EPOCH.add(Duration::from_nanos(
                u64::from_le_bytes(bytes[..8].try_into().expect("This has to be 8 bytes long!")),
            )))
        } else {
            TypedProperty::bytes(bytes)
        }
    }

    pub fn string_map(bytes: Vec<u8>) -> Self {
        TypedProperty::Bytes(bytes.to_vec())
    }

    pub fn boolean(bytes: Vec<u8>) -> Self {
        TypedProperty::Bytes(bytes.to_vec())
    }

    pub fn bytes(bytes: Vec<u8>) -> Self {
        TypedProperty::Bytes(bytes)
    }
}

impl TypedProperty {
    pub fn as_literal(&self) -> String {
        match self {
            TypedProperty::String(str) => {
                format!("\"{}\"", str.replace('\\', "\\\\").replace('"', "\\\""))
            }
            TypedProperty::Integer(int) => int.to_string(),
            TypedProperty::Bytes(bytes) => {
                let len = 5 * bytes.len();
                let mut str = String::with_capacity(len + 1);
                write!(str, "[").unwrap();
                for byte in bytes {
                    write!(str, "\\x{:02x},", byte).unwrap();
                }
                str.replace_range(len..len + 1, "]");
                str
            }
            TypedProperty::Timestamp(ts) => {
                let ts: DateTime<Utc> = (*ts).into();
                ts.to_rfc3339()
            }
        }
    }
}

impl PartialEq<str> for TypedProperty {
    fn eq(&self, other: &str) -> bool {
        match self {
            TypedProperty::String(str) => str == other,
            TypedProperty::Integer(int) => int.to_string().as_str() == other,
            _ => self.as_literal() == other,
        }
    }
}

impl TryFrom<&PatternExpression> for Expression {
    type Error = ();

    fn try_from(expression: &PatternExpression) -> Result<Self, Self::Error> {
        let cel_type = type_of(&expression.selector);

        let value = match cel_type {
            ValueType::Map => match expression.operator {
                WhenConditionOperator::Equal | WhenConditionOperator::NotEqual => {
                    match cel_parser::parse(&expression.value) {
                        Ok(exp) => {
                            if let Expression::Map(data) = exp {
                                Ok(Expression::Map(data))
                            } else {
                                Err(())
                            }
                        }
                        Err(_) => Err(()),
                    }
                }
                _ => Err(()),
            },
            ValueType::Int | ValueType::UInt => match expression.operator {
                WhenConditionOperator::Equal | WhenConditionOperator::NotEqual => {
                    match cel_parser::parse(&expression.value) {
                        Ok(exp) => {
                            if let Expression::Atom(atom) = &exp {
                                match atom {
                                    Atom::Int(_) | Atom::UInt(_) | Atom::Float(_) => Ok(exp),
                                    _ => Err(()),
                                }
                            } else {
                                Err(())
                            }
                        }
                        Err(_) => Err(()),
                    }
                }
                _ => Err(()),
            },
            ValueType::String => match expression.operator {
                WhenConditionOperator::Equal | WhenConditionOperator::NotEqual => {
                    Ok(Expression::Atom(Atom::String(Arc::new(expression.value.clone()))))
                }
                // WhenConditionOperator::Matches => {}
                _ => Ok(Expression::Atom(cel_parser::Atom::String(
                    Arc::new(expression.value.clone()),
                ))),
            },
            // ValueType::Bytes => {}
            // ValueType::Bool => {}
            // ValueType::Timestamp => {}
            _ => todo!("Still needs support for values of type `{cel_type}`"),
        }?;

        match expression.operator {
            WhenConditionOperator::Equal => Ok(Expression::Relation(Ident(Arc::new("attribute".to_string())).into(), RelationOp::Equals, value.into())),
            WhenConditionOperator::NotEqual => Ok(Expression::Relation(Ident(Arc::new("attribute".to_string())).into(), RelationOp::NotEquals, value.into())),
            WhenConditionOperator::StartsWith => Err(()),
            WhenConditionOperator::EndsWith => Err(()),
            WhenConditionOperator::Matches => Err(()),
        }
    }
}

fn type_of(path: &str) -> CelValueType {
    match path {
        "request.time" => CelValueType::Timestamp,
        "request.id" => CelValueType::String,
        "request.protocol" => CelValueType::String,
        "request.scheme" => CelValueType::String,
        "request.host" => CelValueType::String,
        "request.method" => CelValueType::String,
        "request.path" => CelValueType::String,
        "request.url_path" => CelValueType::String,
        "request.query" => CelValueType::String,
        "request.referer" => CelValueType::String,
        "request.useragent" => CelValueType::String,
        "request.body" => CelValueType::String,
        "source.address" => CelValueType::String,
        "source.service" => CelValueType::String,
        "source.principal" => CelValueType::String,
        "source.certificate" => CelValueType::String,
        "destination.address" => CelValueType::String,
        "destination.service" => CelValueType::String,
        "destination.principal" => CelValueType::String,
        "destination.certificate" => CelValueType::String,
        "connection.requested_server_name" => CelValueType::String,
        "connection.tls_session.sni" => CelValueType::String,
        "connection.tls_version" => CelValueType::String,
        "connection.subject_local_certificate" => CelValueType::String,
        "connection.subject_peer_certificate" => CelValueType::String,
        "connection.dns_san_local_certificate" => CelValueType::String,
        "connection.dns_san_peer_certificate" => CelValueType::String,
        "connection.uri_san_local_certificate" => CelValueType::String,
        "connection.uri_san_peer_certificate" => CelValueType::String,
        "connection.sha256_peer_certificate_digest" => CelValueType::String,
        "ratelimit.domain" => CelValueType::String,
        "request.size" => CelValueType::Int,
        "source.port" => CelValueType::Int,
        "destination.port" => CelValueType::Int,
        "connection.id" => CelValueType::Int,
        "ratelimit.hits_addend" => CelValueType::Int,
        "request.headers" => CelValueType::Map,
        "request.context_extensions" => CelValueType::Map,
        "source.labels" => CelValueType::Map,
        "destination.labels" => CelValueType::Map,
        "filter_state" => CelValueType::Map,
        "connection.mtls" => CelValueType::Bool,
        "request.raw_body" => CelValueType::Bytes,
        "auth.identity" => CelValueType::Bytes,
        _ => CelValueType::Bytes,
    }
}

#[cfg(test)]
mod tests {
    use crate::typing::TypedProperty;

    #[test]
    fn literal_bytes() {
        let prop = TypedProperty::bytes(vec![0xb2, 0x42, 0x01]);
        assert_eq!("[\\xb2,\\x42,\\x01]", prop.as_literal());
    }

    #[test]
    fn literal_strings() {
        let prop = TypedProperty::String("Foobar".to_string());
        assert_eq!("\"Foobar\"", prop.as_literal());
        let prop = TypedProperty::String("Foo\\bar".to_string());
        assert_eq!("\"Foo\\\\bar\"", prop.as_literal());
        let prop = TypedProperty::String("Foo\"bar\"".to_string());
        assert_eq!("\"Foo\\\"bar\\\"\"", prop.as_literal());
        let prop = TypedProperty::String("\\Foo\"bar\"".to_string());
        assert_eq!("\"\\\\Foo\\\"bar\\\"\"", prop.as_literal());
    }
}
