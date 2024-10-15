use crate::attribute::Attribute;
use crate::configuration::{DataItem, DataType};
use crate::envoy::{RateLimitDescriptor, RateLimitDescriptor_Entry};
use log::debug;
use protobuf::RepeatedField;
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub extension: String,
    pub scope: String,
    #[serde(default)]
    pub data: Vec<DataItem>,
}

impl Action {
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
            match &data.item {
                DataType::Static(static_item) => {
                    let mut descriptor_entry = RateLimitDescriptor_Entry::new();
                    descriptor_entry.set_key(static_item.key.to_owned());
                    descriptor_entry.set_value(static_item.value.to_owned());
                    entries.push(descriptor_entry);
                }
                DataType::Selector(selector_item) => {
                    let descriptor_key = match &selector_item.key {
                        None => selector_item.path().to_string(),
                        Some(key) => key.to_owned(),
                    };

                    let attribute_path = selector_item.path();
                    let value = match crate::property::get_property(attribute_path.tokens())
                        .unwrap()
                    {
                        //TODO(didierofrivia): Replace hostcalls by DI
                        None => {
                            debug!(
                                "build_single_descriptor: selector not found: {}",
                                attribute_path
                            );
                            match &selector_item.default {
                                None => return None, // skipping the entire descriptor
                                Some(default_value) => default_value.clone(),
                            }
                        }
                        // TODO(eastizle): not all fields are strings
                        // https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/advanced/attributes
                        Some(attribute_bytes) => match Attribute::parse(attribute_bytes) {
                            Ok(attr_str) => attr_str,
                            Err(e) => {
                                debug!("build_single_descriptor: failed to parse selector value: {}, error: {}",
                                    attribute_path, e);
                                return None;
                            }
                        },
                        // Alternative implementation (for rust >= 1.76)
                        // Attribute::parse(attribute_bytes)
                        //   .inspect_err(|e| debug!("#{} build_single_descriptor: failed to parse selector value: {}, error: {}",
                        //           filter.context_id, attribute_path, e))
                        //   .ok()?,
                    };
                    let mut descriptor_entry = RateLimitDescriptor_Entry::new();
                    descriptor_entry.set_key(descriptor_key);
                    descriptor_entry.set_value(value);
                    entries.push(descriptor_entry);
                }
            }
        }
        let mut res = RateLimitDescriptor::new();
        res.set_entries(entries);
        Some(res)
    }
}
