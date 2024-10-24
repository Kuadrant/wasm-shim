use crate::configuration::{DataItem, DataType, PatternExpression};
use crate::data::Predicate;
use crate::envoy::{RateLimitDescriptor, RateLimitDescriptor_Entry};
use cel_interpreter::Value;
use log::debug;
use protobuf::RepeatedField;
use serde::Deserialize;
use std::cell::OnceCell;

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub service: String,
    pub scope: String,
    #[serde(default)]
    pub conditions: Vec<PatternExpression>,
    #[serde(default)]
    pub predicates: Vec<String>,
    #[serde(skip_deserializing)]
    pub compiled_predicates: OnceCell<Vec<Predicate>>,
    #[serde(default)]
    pub data: Vec<DataItem>,
}

impl Action {
    pub fn conditions_apply(&self) -> bool {
        let predicates = self
            .compiled_predicates
            .get()
            .expect("predicates must be compiled by now");
        if predicates.is_empty() {
            self.conditions.is_empty() || self.conditions.iter().all(PatternExpression::applies)
        } else {
            predicates.iter().all(Predicate::test)
        }
    }

    pub fn build_descriptors(&self) -> RepeatedField<RateLimitDescriptor> {
        let mut entries = RepeatedField::new();
        if let Some(desc) = self.build_single_descriptor() {
            entries.push(desc);
        }
        entries
    }

    fn build_single_descriptor(&self) -> Option<RateLimitDescriptor> {
        let mut entries = RepeatedField::default();

        // iterate over data items to allow any data item to skip the entire descriptor
        for data in self.data.iter() {
            let (key, value) = match &data.item {
                DataType::Static(static_item) => {
                    (static_item.key.to_owned(), static_item.value.to_owned())
                }
                DataType::Expression(cel) => (
                    cel.key.clone(),
                    match cel
                        .compiled
                        .get()
                        .expect("Expression must be compiled by now")
                        .eval()
                    {
                        Value::Int(n) => format!("{n}"),
                        Value::UInt(n) => format!("{n}"),
                        Value::Float(n) => format!("{n}"),
                        // todo this probably should be a proper string literal!
                        Value::String(s) => (*s).clone(),
                        Value::Bool(b) => format!("{b}"),
                        Value::Null => "null".to_owned(),
                        _ => panic!("Only scalar values can be sent as data"),
                    },
                ),
                DataType::Selector(selector_item) => {
                    let descriptor_key = match &selector_item.key {
                        None => selector_item.path().to_string(),
                        Some(key) => key.to_owned(),
                    };

                    let value = match crate::data::get_attribute::<String>(selector_item.path())
                        .expect("Error!")
                    {
                        //TODO(didierofrivia): Replace hostcalls by DI
                        None => {
                            debug!(
                                "build_single_descriptor: selector not found: {}",
                                selector_item.path()
                            );
                            match &selector_item.default {
                                None => return None, // skipping the entire descriptor
                                Some(default_value) => default_value.clone(),
                            }
                        }
                        // TODO(eastizle): not all fields are strings
                        // https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/advanced/attributes
                        Some(attr_str) => attr_str,
                        // Alternative implementation (for rust >= 1.76)
                        // Attribute::parse(attribute_bytes)
                        //   .inspect_err(|e| debug!("#{} build_single_descriptor: failed to parse selector value: {}, error: {}",
                        //           filter.context_id, attribute_path, e))
                        //   .ok()?,
                    };
                    (descriptor_key, value)
                }
            };
            let mut descriptor_entry = RateLimitDescriptor_Entry::new();
            descriptor_entry.set_key(key);
            descriptor_entry.set_value(value);
            entries.push(descriptor_entry);
        }
        let mut res = RateLimitDescriptor::new();
        res.set_entries(entries);
        Some(res)
    }
}
