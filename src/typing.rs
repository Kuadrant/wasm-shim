use chrono::{DateTime, Utc};
use std::fmt::Write;
use std::ops::Add;
use std::time::{Duration, SystemTime};

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
    pub fn as_string(&self) -> String {
        match self {
            TypedProperty::String(str) => str.clone(),
            TypedProperty::Integer(int) => int.to_string(),
            _ => self.as_literal(),
        }
    }

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

#[cfg(test)]
mod tests {
    use crate::typing::TypedProperty;

    #[test]
    fn escapes_bytes() {
        let prop = TypedProperty::bytes(vec![0xb2, 0x42, 0x01]);
        assert_eq!(prop.as_literal(), prop.as_string());
    }

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
