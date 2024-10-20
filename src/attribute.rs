use crate::property_path::Path;
use chrono::{DateTime, FixedOffset};
use log::{debug, error};
use protobuf::well_known_types::Struct;
use proxy_wasm::hostcalls;

pub const KUADRANT_NAMESPACE: &str = "kuadrant";

pub trait Attribute {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, String>
    where
        Self: Sized;
}

impl Attribute for String {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, String> {
        String::from_utf8(raw_attribute).map_err(|err| {
            format!(
                "parse: failed to parse selector String value, error: {}",
                err
            )
        })
    }
}

impl Attribute for i64 {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, String> {
        if raw_attribute.len() != 8 {
            return Err(format!(
                "parse: Int value expected to be 8 bytes, but got {}",
                raw_attribute.len()
            ));
        }
        Ok(i64::from_le_bytes(
            raw_attribute[..8]
                .try_into()
                .expect("This has to be 8 bytes long!"),
        ))
    }
}

impl Attribute for u64 {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, String> {
        if raw_attribute.len() != 8 {
            return Err(format!(
                "parse: UInt value expected to be 8 bytes, but got {}",
                raw_attribute.len()
            ));
        }
        Ok(u64::from_le_bytes(
            raw_attribute[..8]
                .try_into()
                .expect("This has to be 8 bytes long!"),
        ))
    }
}

impl Attribute for f64 {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, String> {
        if raw_attribute.len() != 8 {
            return Err(format!(
                "parse: Float value expected to be 8 bytes, but got {}",
                raw_attribute.len()
            ));
        }
        Ok(f64::from_le_bytes(
            raw_attribute[..8]
                .try_into()
                .expect("This has to be 8 bytes long!"),
        ))
    }
}

impl Attribute for Vec<u8> {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, String> {
        Ok(raw_attribute)
    }
}

impl Attribute for bool {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, String> {
        if raw_attribute.len() != 1 {
            return Err(format!(
                "parse: Bool value expected to be 1 byte, but got {}",
                raw_attribute.len()
            ));
        }
        Ok(raw_attribute[0] & 1 == 1)
    }
}

impl Attribute for DateTime<FixedOffset> {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, String> {
        if raw_attribute.len() != 8 {
            return Err(format!(
                "parse: Timestamp expected to be 8 bytes, but got {}",
                raw_attribute.len()
            ));
        }

        let nanos = i64::from_le_bytes(
            raw_attribute.as_slice()[..8]
                .try_into()
                .expect("This has to be 8 bytes long!"),
        );
        Ok(DateTime::from_timestamp_nanos(nanos).into())
    }
}

pub fn get_attribute<T>(attr: &str) -> Result<T, String>
where
    T: Attribute,
{
    match crate::property::get_property(Path::from(attr).tokens()) {
        Ok(Some(attribute_bytes)) => T::parse(attribute_bytes),
        Ok(None) => Err(format!("get_attribute: not found or null: {attr}")),
        Err(e) => Err(format!("get_attribute: error: {e:?}")),
    }
}

pub fn set_attribute(attr: &str, value: &[u8]) {
    match hostcalls::set_property(Path::from(attr).tokens(), Some(value)) {
        Ok(_) => (),
        Err(_) => error!("set_attribute: failed to set property {attr}"),
    };
}

pub fn store_metadata(metastruct: &Struct) {
    let metadata = process_metadata(metastruct, String::new());
    for (key, value) in metadata {
        let attr = format!("{KUADRANT_NAMESPACE}\\.{key}");
        debug!("set_attribute: {attr} = {value}");
        set_attribute(attr.as_str(), value.into_bytes().as_slice());
    }
}

fn process_metadata(s: &Struct, prefix: String) -> Vec<(String, String)> {
    let mut result = Vec::new();
    for (key, value) in s.get_fields() {
        let current_prefix = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}\\.{key}")
        };

        if value.has_string_value() {
            result.push((current_prefix, value.get_string_value().to_string()));
        } else if value.has_struct_value() {
            let nested_struct = value.get_struct_value();
            result.extend(process_metadata(nested_struct, current_prefix));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use crate::attribute::process_metadata;
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
            vec![("identity\\.userid".to_string(), "bob".to_string())]
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
        assert!(output.contains(&("identity\\.userid".to_string(), "bob".to_string())));
        assert!(output.contains(&("identity\\.type".to_string(), "test".to_string())));
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
        assert_eq!(output.len(), 2);
        assert!(output.contains(&("identity\\.userid".to_string(), "bob".to_string())));
        assert!(output.contains(&("other_data".to_string(), "other_value".to_string())));
    }
}
