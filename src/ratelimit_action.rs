use crate::configuration::{Action, DataType, FailureMode, Service};
use crate::data::Expression;
use crate::data::Predicate;
use crate::envoy::{RateLimitDescriptor, RateLimitDescriptor_Entry};
use crate::service::GrpcService;
use cel_interpreter::Value;
use log::error;
use protobuf::RepeatedField;
use std::rc::Rc;

#[derive(Debug)]
struct DescriptorEntryBuilder {
    pub key: String,
    pub expression: Expression,
}

impl DescriptorEntryBuilder {
    pub fn new(data_type: &DataType) -> Result<Self, String> {
        match data_type {
            DataType::Static(static_item) => Ok(DescriptorEntryBuilder {
                key: static_item.key.clone(),
                expression: Expression::new(format!("'{}'", static_item.value).as_str())
                    .map_err(|e| e.to_string())?,
            }),
            DataType::Expression(exp_item) => Ok(DescriptorEntryBuilder {
                key: exp_item.key.clone(),
                expression: Expression::new(&exp_item.value).map_err(|e| e.to_string())?,
            }),
        }
    }

    pub fn evaluate(&self) -> RateLimitDescriptor_Entry {
        let (key, value) = (
            self.key.clone(),
            match self.expression.eval() {
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
                    error!("Failed to evaluate {:?}: {}", self.expression, err);
                    panic!("Err out of this!")
                }
            },
        );
        let mut descriptor_entry = RateLimitDescriptor_Entry::new();
        descriptor_entry.set_key(key);
        descriptor_entry.set_value(value);
        descriptor_entry
    }
}

#[derive(Debug)]
struct ConditionalData {
    pub data: Vec<DescriptorEntryBuilder>,
    pub predicates: Vec<Predicate>,
}

impl ConditionalData {
    pub fn new(action: &Action) -> Result<Self, String> {
        let mut predicates = Vec::default();
        for predicate in &action.predicates {
            predicates.push(Predicate::new(predicate).map_err(|e| e.to_string())?);
        }

        let mut data = Vec::default();
        for datum in &action.data {
            data.push(DescriptorEntryBuilder::new(&datum.item)?);
        }
        Ok(ConditionalData { data, predicates })
    }

    fn predicates_apply(&self) -> bool {
        let predicates = &self.predicates;
        predicates.is_empty()
            || predicates.iter().all(|predicate| match predicate.test() {
                Ok(b) => b,
                Err(err) => {
                    error!("Failed to evaluate {:?}: {}", predicates, err);
                    panic!("Err out of this!")
                }
            })
    }

    pub fn entries(&self) -> RepeatedField<RateLimitDescriptor_Entry> {
        if !self.predicates_apply() {
            return RepeatedField::default();
        }

        let mut entries = RepeatedField::default();
        for entry_builder in self.data.iter() {
            entries.push(entry_builder.evaluate());
        }

        entries
    }
}

#[derive(Debug)]
pub struct RateLimitAction {
    grpc_service: Rc<GrpcService>,
    scope: String,
    service_name: String,
    conditional_data_sets: Vec<ConditionalData>,
}

impl RateLimitAction {
    pub fn new(action: &Action, service: &Service) -> Result<Self, String> {
        Ok(Self {
            grpc_service: Rc::new(GrpcService::new(Rc::new(service.clone()))),
            scope: action.scope.clone(),
            service_name: action.service.clone(),
            conditional_data_sets: vec![ConditionalData::new(action)?],
        })
    }

    pub fn build_descriptor(&self) -> RateLimitDescriptor {
        let mut entries = RepeatedField::default();

        for conditional_data in self.conditional_data_sets.iter() {
            entries.extend(conditional_data.entries());
        }

        let mut res = RateLimitDescriptor::new();
        res.set_entries(entries);
        res
    }

    pub fn get_grpcservice(&self) -> Rc<GrpcService> {
        Rc::clone(&self.grpc_service)
    }

    pub fn scope(&self) -> &str {
        self.scope.as_str()
    }

    pub fn conditions_apply(&self) -> bool {
        // For RateLimitAction conditions always apply.
        // It is when building the descriptor that it may be empty because predicates do not
        // evaluate to true.
        true
    }

    pub fn get_failure_mode(&self) -> FailureMode {
        self.grpc_service.get_failure_mode()
    }

    #[must_use]
    pub fn merge(&mut self, other: RateLimitAction) -> Option<RateLimitAction> {
        if self.scope == other.scope && self.service_name == other.service_name {
            self.conditional_data_sets
                .extend(other.conditional_data_sets);
            return None;
        }
        Some(other)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::configuration::{
        Action, DataItem, DataType, ExpressionItem, FailureMode, Service, ServiceType, StaticItem,
        Timeout,
    };

    fn build_service() -> Service {
        Service {
            service_type: ServiceType::RateLimit,
            endpoint: "some_endpoint".into(),
            failure_mode: FailureMode::default(),
            timeout: Timeout::default(),
        }
    }

    fn build_action(predicates: Vec<String>, data: Vec<DataItem>) -> Action {
        Action {
            service: "some_service".into(),
            scope: "some_scope".into(),
            predicates,
            data,
        }
    }

    #[test]
    fn empty_predicates_do_apply() {
        let action = build_action(Vec::default(), Vec::default());
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service)
            .expect("action building failed. Maybe predicates compilation?");
        assert!(rl_action.conditions_apply());
    }

    #[test]
    fn even_with_falsy_predicates_conditions_apply() {
        let action = build_action(vec!["false".into()], Vec::default());
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service)
            .expect("action building failed. Maybe predicates compilation?");
        assert!(rl_action.conditions_apply());
    }

    #[test]
    fn empty_data_generates_empty_descriptor() {
        let action = build_action(Vec::default(), Vec::default());
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service)
            .expect("action building failed. Maybe predicates compilation?");
        assert_eq!(rl_action.build_descriptor(), RateLimitDescriptor::default());
    }

    #[test]
    fn descriptor_entry_from_expression() {
        let data = vec![DataItem {
            item: DataType::Expression(ExpressionItem {
                key: "key_1".into(),
                value: "'value_1'".into(),
            }),
        }];
        let action = build_action(Vec::default(), data);
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service)
            .expect("action building failed. Maybe predicates compilation?");
        let descriptor = rl_action.build_descriptor();
        assert_eq!(descriptor.get_entries().len(), 1);
        assert_eq!(descriptor.get_entries()[0].key, String::from("key_1"));
        assert_eq!(descriptor.get_entries()[0].value, String::from("value_1"));
    }

    #[test]
    fn descriptor_entry_from_static() {
        let data = vec![DataItem {
            item: DataType::Static(StaticItem {
                key: "key_1".into(),
                value: "value_1".into(),
            }),
        }];
        let action = build_action(Vec::default(), data);
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service)
            .expect("action building failed. Maybe predicates compilation?");
        let descriptor = rl_action.build_descriptor();
        assert_eq!(descriptor.get_entries().len(), 1);
        assert_eq!(descriptor.get_entries()[0].key, String::from("key_1"));
        assert_eq!(descriptor.get_entries()[0].value, String::from("value_1"));
    }

    #[test]
    fn descriptor_entries_not_generated_when_predicates_evaluate_to_false() {
        let data = vec![DataItem {
            item: DataType::Expression(ExpressionItem {
                key: "key_1".into(),
                value: "'value_1'".into(),
            }),
        }];

        let predicates = vec!["false".into(), "true".into()];
        let action = build_action(predicates, data);
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service)
            .expect("action building failed. Maybe predicates compilation?");
        assert_eq!(rl_action.build_descriptor(), RateLimitDescriptor::default());
    }

    #[test]
    fn merged_actions_generate_descriptor_entries_for_truthy_predicates() {
        let service = build_service();

        let data_1 = vec![DataItem {
            item: DataType::Expression(ExpressionItem {
                key: "key_1".into(),
                value: "'value_1'".into(),
            }),
        }];
        let predicates_1 = vec!["true".into()];
        let action_1 = build_action(predicates_1, data_1);
        let mut rl_action_1 = RateLimitAction::new(&action_1, &service)
            .expect("action building failed. Maybe predicates compilation?");

        let data_2 = vec![DataItem {
            item: DataType::Expression(ExpressionItem {
                key: "key_2".into(),
                value: "'value_2'".into(),
            }),
        }];
        let predicates_2 = vec!["false".into()];
        let action_2 = build_action(predicates_2, data_2);
        let rl_action_2 = RateLimitAction::new(&action_2, &service)
            .expect("action building failed. Maybe predicates compilation?");

        let data_3 = vec![DataItem {
            item: DataType::Expression(ExpressionItem {
                key: "key_3".into(),
                value: "'value_3'".into(),
            }),
        }];
        let predicates_3 = vec!["true".into()];
        let action_3 = build_action(predicates_3, data_3);
        let rl_action_3 = RateLimitAction::new(&action_3, &service)
            .expect("action building failed. Maybe predicates compilation?");

        assert!(rl_action_1.merge(rl_action_2).is_none());
        assert!(rl_action_1.merge(rl_action_3).is_none());

        // it should generate descriptor entries from action 1 and action 3

        let descriptor = rl_action_1.build_descriptor();
        assert_eq!(descriptor.get_entries().len(), 2);
        assert_eq!(descriptor.get_entries()[0].key, String::from("key_1"));
        assert_eq!(descriptor.get_entries()[0].value, String::from("value_1"));
        assert_eq!(descriptor.get_entries()[1].key, String::from("key_3"));
        assert_eq!(descriptor.get_entries()[1].value, String::from("value_3"));
    }
}
