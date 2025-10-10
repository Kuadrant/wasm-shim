use crate::data::PropertyPath;
use crate::v2::data::attribute::{AttributeError, AttributeValue};
use log::{debug, error, warn};
use prost_types::value::Kind;
use prost_types::Struct;
use serde_json::Value;

pub const KUADRANT_NAMESPACE: &str = "kuadrant";

pub fn get_attribute<T>(path: &PropertyPath) -> Result<Option<T>, AttributeError>
where
    T: AttributeValue,
{
    match crate::data::property::get_property(path) {
        Ok(Some(attribute_bytes)) => Ok(Some(T::parse(attribute_bytes)?)),
        Ok(None) => Ok(None),
        Err(e) => Err(AttributeError::Retrieval(format!(
            "get_attribute: error: {e:?}"
        ))),
    }
}

pub fn set_attribute(attr: &str, value: &[u8]) -> Result<(), AttributeError> {
    crate::data::property::set_property(PropertyPath::from(attr), Some(value))
        .map_err(|e| AttributeError::Retrieval(format!("set_attribute: error: {e:?}")))
}

pub fn store_metadata(metastruct: &Struct) -> Result<(), AttributeError> {
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
    for (key, value) in &s.fields {
        let current_prefix = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}\\.{key}")
        };

        match &value.kind {
            Some(Kind::StructValue(nested_struct)) => {
                result.extend(process_metadata(nested_struct, current_prefix));
            }
            Some(kind) => {
                let json = match kind {
                    Kind::StringValue(s) => Some(s.clone().into()),
                    Kind::BoolValue(b) => Some((*b).into()),
                    Kind::NullValue(_) => Some(Value::Null),
                    Kind::NumberValue(n) => Some((*n).into()),
                    Kind::StructValue(_) => unreachable!(), // handled above
                    _ => {
                        warn!(
                            "Don't know how to store Struct field `{}` of kind {:?}",
                            key, kind
                        );
                        None
                    }
                };

                if let Some(v) = json {
                    match serde_json::to_string(&v) {
                        Ok(ser) => result.push((current_prefix, ser)),
                        Err(e) => error!("failed to serialize json Value: {e:?}"),
                    }
                }
            }
            None => {
                warn!("Struct field `{}` has no kind", key);
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use crate::data::attribute::process_metadata;
    use prost_types::{Struct, Value};
    use std::collections::BTreeMap;

    pub fn struct_from(values: Vec<(String, Value)>) -> Struct {
        let mut hm = BTreeMap::new();
        for (key, value) in values {
            hm.insert(key, value);
        }
        Struct { fields: hm }
    }

    pub fn string_value_from(value: String) -> Value {
        Value {
            kind: Some(prost_types::value::Kind::StringValue(value)),
        }
    }

    pub fn struct_value_from(value: Struct) -> Value {
        Value {
            kind: Some(prost_types::value::Kind::StructValue(value)),
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
