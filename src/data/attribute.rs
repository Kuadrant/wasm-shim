use crate::data::attribute::errors::PropError;
use crate::data::{PropertyError, PropertyPath};
use chrono::{DateTime, FixedOffset};
use log::{debug, error, warn};
use protobuf::well_known_types::Struct;
use serde_json::Value;

pub const KUADRANT_NAMESPACE: &str = "kuadrant";

pub(super) mod errors {
    use std::error::Error;
    use std::fmt::{Debug, Display, Formatter};

    #[derive(Debug, PartialEq)]
    pub enum PropertyError {
        Get(PropError),
        Parse(PropError),
        RequestBodyNotAvailable,
    }

    impl Error for PropertyError {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            match self {
                PropertyError::Get(err) => Some(err),
                PropertyError::Parse(err) => Some(err),
                PropertyError::RequestBodyNotAvailable => None,
            }
        }
    }

    impl Display for PropertyError {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            match self {
                PropertyError::Get(e) => {
                    write!(f, "PropertyError::Get {{ {e:?} }}")
                }
                PropertyError::Parse(e) => {
                    write!(f, "PropertyError::Parse {{ {e:?} }}")
                }
                PropertyError::RequestBodyNotAvailable => {
                    write!(f, "PropertyError::RequestBodyNotAvailable")
                }
            }
        }
    }

    #[derive(Debug, PartialEq)]
    pub struct PropError {
        message: String,
    }

    impl Display for PropError {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            write!(f, "PropError {{ message: {:?} }}", self.message)
        }
    }

    impl Error for PropError {}

    impl PropError {
        pub fn new(message: String) -> PropError {
            PropError { message }
        }
    }
}

pub trait AttributeValue {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, PropError>
    where
        Self: Sized;
}

impl AttributeValue for String {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, PropError> {
        String::from_utf8(raw_attribute).map_err(|err| {
            PropError::new(format!(
                "parse: failed to parse selector String value, error: {err}"
            ))
        })
    }
}

impl AttributeValue for i64 {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, PropError> {
        let ra_len = raw_attribute.len();
        match <[u8; 8]>::try_from(raw_attribute) {
            Ok(bytes) => Ok(i64::from_le_bytes(bytes)),
            Err(_) => Err(PropError::new(format!(
                "parse: Int value expected to be 8 bytes, but got {ra_len}",
            ))),
        }
    }
}

impl AttributeValue for u64 {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, PropError> {
        let ra_len = raw_attribute.len();
        match <[u8; 8]>::try_from(raw_attribute) {
            Ok(bytes) => Ok(u64::from_le_bytes(bytes)),
            Err(_) => Err(PropError::new(format!(
                "parse: UInt value expected to be 8 bytes, but got {ra_len}",
            ))),
        }
    }
}

impl AttributeValue for f64 {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, PropError> {
        let ra_len = raw_attribute.len();
        match <[u8; 8]>::try_from(raw_attribute) {
            Ok(bytes) => Ok(f64::from_le_bytes(bytes)),
            Err(_) => Err(PropError::new(format!(
                "parse: Float value expected to be 8 bytes, but got {ra_len}",
            ))),
        }
    }
}

impl AttributeValue for Vec<u8> {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, PropError> {
        Ok(raw_attribute)
    }
}

impl AttributeValue for bool {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, PropError> {
        if raw_attribute.len() == 1 {
            return Ok(raw_attribute[0] & 1 == 1);
        }
        Err(PropError::new(format!(
            "parse: Bool value expected to be 1 byte, but got {}",
            raw_attribute.len()
        )))
    }
}

impl AttributeValue for DateTime<FixedOffset> {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, PropError> {
        let ra_len = raw_attribute.len();
        match <[u8; 8]>::try_from(raw_attribute) {
            Ok(bytes) => {
                let nanos = i64::from_le_bytes(bytes);
                Ok(DateTime::from_timestamp_nanos(nanos).into())
            }
            Err(_) => Err(PropError::new(format!(
                "parse: Timestamp expected to be 8 bytes, but got {ra_len}",
            ))),
        }
    }
}

pub fn get_attribute<T>(path: &PropertyPath) -> Result<Option<T>, PropertyError>
where
    T: AttributeValue,
{
    match crate::data::property::get_property(path) {
        Ok(Some(attribute_bytes)) => Ok(Some(
            T::parse(attribute_bytes).map_err(PropertyError::Parse)?,
        )),
        Ok(None) => Ok(None),
        Err(e) => Err(PropertyError::Get(PropError::new(format!(
            "get_attribute: error: {e:?}"
        )))),
    }
}

pub fn set_attribute(attr: &str, value: &[u8]) -> Result<(), PropertyError> {
    crate::data::property::set_property(PropertyPath::from(attr), Some(value))
        .map_err(|e| PropertyError::Get(PropError::new(format!("set_attribute: error: {e:?}"))))
}

pub fn store_metadata(metastruct: &Struct) -> Result<(), PropertyError> {
    let metadata = process_metadata(metastruct, String::new());
    for (key, value) in metadata {
        let attr = format!("{KUADRANT_NAMESPACE}\\.auth\\.{key}");
        debug!("set_attribute: {attr} = {value}");
        if let Err(e) = set_attribute(attr.as_str(), value.into_bytes().as_slice()) {
            error!("set_attribute: failed to set property {attr}: {e:?}");
            return Err(e);
        }
    }
    Ok(())
}

fn process_metadata(s: &Struct, prefix: String) -> Vec<(String, String)> {
    let mut result = Vec::new();
    for (key, value) in s.get_fields() {
        let current_prefix = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}\\.{key}")
        };

        let json: Option<Value> = if value.has_string_value() {
            Some(value.get_string_value().into())
        } else if value.has_bool_value() {
            Some(value.get_bool_value().into())
        } else if value.has_null_value() {
            Some(Value::Null)
        } else if value.has_number_value() {
            Some(value.get_number_value().into())
        } else {
            if !value.has_struct_value() {
                warn!(
                    "Don't know how to store Struct field `{}` of kind {:?}",
                    key, value.kind
                );
            }
            None
        };

        if value.has_struct_value() {
            let nested_struct = value.get_struct_value();
            result.extend(process_metadata(nested_struct, current_prefix));
        } else if let Some(v) = json {
            match serde_json::to_string(&v) {
                Ok(ser) => result.push((current_prefix, ser)),
                Err(e) => error!("failed to serialize json Value: {e:?}"),
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use crate::data::attribute::process_metadata;
    use protobuf::well_known_types::{Struct, Value, Value_oneof_kind};
    use std::collections::HashMap;

    pub fn struct_from(values: Vec<(String, Value)>) -> Struct {
        let mut hm = HashMap::new();
        for (key, value) in values {
            hm.insert(key, value);
        }
        Struct {
            fields: hm,
            unknown_fields: Default::default(),
            cached_size: Default::default(),
        }
    }

    pub fn string_value_from(value: String) -> Value {
        Value {
            kind: Some(Value_oneof_kind::string_value(value)),
            unknown_fields: Default::default(),
            cached_size: Default::default(),
        }
    }

    pub fn struct_value_from(value: Struct) -> Value {
        Value {
            kind: Some(Value_oneof_kind::struct_value(value)),
            unknown_fields: Default::default(),
            cached_size: Default::default(),
        }
    }
    #[test]
    fn get_metadata_one() {
        let metadata = struct_from(vec![(
            "identity".to_string(),
            struct_value_from(struct_from(vec![(
                "userid".to_string(),
                string_value_from("bob".to_string()),
            )])),
        )]);
        let output = process_metadata(&metadata, String::new());
        assert_eq!(output.len(), 1);
        assert_eq!(
            output,
            vec![("identity\\.userid".to_string(), "\"bob\"".to_string())]
        );
    }

    #[test]
    fn get_metadata_two() {
        let metadata = struct_from(vec![(
            "identity".to_string(),
            struct_value_from(struct_from(vec![
                ("userid".to_string(), string_value_from("bob".to_string())),
                ("type".to_string(), string_value_from("test".to_string())),
            ])),
        )]);
        let output = process_metadata(&metadata, String::new());
        assert_eq!(output.len(), 2);
        println!("{output:#?}");
        assert!(output.contains(&("identity\\.userid".to_string(), "\"bob\"".to_string())));
        assert!(output.contains(&("identity\\.type".to_string(), "\"test\"".to_string())));
    }

    #[test]
    fn get_metadata_three() {
        let metadata = struct_from(vec![
            (
                "identity".to_string(),
                struct_value_from(struct_from(vec![(
                    "userid".to_string(),
                    string_value_from("bob".to_string()),
                )])),
            ),
            (
                "other_data".to_string(),
                string_value_from("other_value".to_string()),
            ),
        ]);
        let output = process_metadata(&metadata, String::new());
        println!("{output:#?}");
        assert_eq!(output.len(), 2);
        assert!(output.contains(&("identity\\.userid".to_string(), "\"bob\"".to_string())));
        assert!(output.contains(&("other_data".to_string(), "\"other_value\"".to_string())));
    }
}
