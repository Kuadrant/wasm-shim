use crate::configuration::{DataItem, DataType};
use crate::data::Predicate;
use crate::envoy::{RateLimitDescriptor, RateLimitDescriptor_Entry};
use cel_interpreter::Value;
use log::error;
use protobuf::RepeatedField;
use serde::Deserialize;
use std::cell::OnceCell;

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub service: String,
    pub scope: String,
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
        predicates.is_empty()
            || predicates
                .iter()
                .enumerate()
                .all(|(pos, predicate)| match predicate.test() {
                    Ok(b) => b,
                    Err(err) => {
                        error!("Failed to evaluate {}: {}", self.predicates[pos], err);
                        panic!("Err out of this!")
                    }
                })
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
                        Ok(value) => match value {
                            Value::Int(n) => format!("{n}"),
                            Value::UInt(n) => format!("{n}"),
                            Value::Float(n) => format!("{n}"),
                            // todo this probably should be a proper string literal!
                            Value::String(s) => (*s).clone(),
                            Value::Bool(b) => format!("{b}"),
                            Value::Null => "null".to_owned(),
                            _ => panic!("Only scalar values can be sent as data"),
                        },
                        Err(err) => {
                            error!("Failed to evaluate {}: {}", cel.value, err);
                            panic!("Err out of this!")
                        }
                    },
                ),
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

#[cfg(test)]
mod test {
    use crate::configuration::action::Action;
    use std::cell::OnceCell;

    #[test]
    fn empty_predicates_do_apply() {
        let compiled_predicates = OnceCell::new();
        compiled_predicates
            .set(Vec::default())
            .expect("predicates must not be compiled yet!");

        let action = Action {
            service: String::from("svc1"),
            scope: String::from("sc1"),
            predicates: vec![],
            compiled_predicates,
            data: vec![],
        };

        assert!(action.conditions_apply())
    }
}
