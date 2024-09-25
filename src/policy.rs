use crate::attribute::Attribute;
use crate::configuration::{Action, DataItem, DataType, PatternExpression};
use crate::envoy::{RateLimitDescriptor, RateLimitDescriptor_Entry};
use log::debug;
use protobuf::RepeatedField;
use proxy_wasm::hostcalls;
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Condition {
    pub all_of: Vec<PatternExpression>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Rule {
    //
    #[serde(default)]
    pub conditions: Vec<Condition>,
    //
    pub actions: Vec<Action>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Policy {
    pub name: String,
    pub domain: String,
    pub hostnames: Vec<String>,
    pub rules: Vec<Rule>,
}

impl Policy {
    #[cfg(test)]
    pub fn new(name: String, domain: String, hostnames: Vec<String>, rules: Vec<Rule>) -> Self {
        Policy {
            name,
            domain,
            hostnames,
            rules,
        }
    }

    pub fn find_rule_that_applies(&self) -> Option<&Rule> {
        self.rules
            .iter()
            .find(|rule: &&Rule| self.filter_rule_by_conditions(&rule.conditions))
    }

    pub fn build_descriptors(&self, rule: &Rule) -> RepeatedField<RateLimitDescriptor> {
        rule.actions
            .iter()
            .filter_map(|action| self.build_single_descriptor(&action.data))
            .collect()
    }

    fn filter_rule_by_conditions(&self, conditions: &[Condition]) -> bool {
        if conditions.is_empty() {
            // no conditions is equivalent to matching all the requests.
            return true;
        }

        conditions
            .iter()
            .any(|condition| self.condition_applies(condition))
    }

    fn condition_applies(&self, condition: &Condition) -> bool {
        condition
            .all_of
            .iter()
            .all(|pattern_expression| self.pattern_expression_applies(pattern_expression))
    }

    fn pattern_expression_applies(&self, p_e: &PatternExpression) -> bool {
        let attribute_path = p_e.path();
        debug!(
            "get_property:  selector: {} path: {:?}",
            p_e.selector, attribute_path
        );
        let attribute_value = match hostcalls::get_property(attribute_path).unwrap() {
            //TODO(didierofrivia): Replace hostcalls by DI
            None => {
                debug!(
                    "pattern_expression_applies:  selector not found: {}, defaulting to ``",
                    p_e.selector
                );
                b"".to_vec()
            }
            Some(attribute_bytes) => attribute_bytes,
        };
        match p_e.eval(attribute_value) {
            Err(e) => {
                debug!("pattern_expression_applies failed: {}", e);
                false
            }
            Ok(result) => result,
        }
    }

    fn build_single_descriptor(&self, data_list: &[DataItem]) -> Option<RateLimitDescriptor> {
        let mut entries = ::protobuf::RepeatedField::default();

        // iterate over data items to allow any data item to skip the entire descriptor
        for data in data_list.iter() {
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
                    debug!(
                        "get_property:  selector: {} path: {:?}",
                        selector_item.selector, attribute_path
                    );
                    let value = match hostcalls::get_property(attribute_path.tokens()).unwrap() {
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
