use crate::configuration::{Action, DataType, FailureMode, Service};
use crate::data::{EvaluationError, Predicate};
use crate::data::{Expression, PredicateResult};
use crate::envoy::{
    HeaderValue, RateLimitDescriptor, RateLimitDescriptor_Entry, RateLimitResponse,
    RateLimitResponse_Code, StatusCode,
};
use crate::runtime_action::errors::ActionCreationError;
use crate::runtime_action::ResponseResult;
use crate::service::{GrpcErrResponse, GrpcService, HeaderKind, Headers};
use cel_interpreter::Value;
use cel_parser::ParseError;
use log::{debug, error};
use protobuf::RepeatedField;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Debug, PartialEq, Clone)]
struct DescriptorEntryBuilder {
    pub key: String,
    pub expression: Expression,
}

const KNOWN_ATTRIBUTES: [&str; 2] = ["ratelimit.domain", "ratelimit.hits_addend"];

impl DescriptorEntryBuilder {
    pub fn new(data_type: &DataType) -> Result<Self, ParseError> {
        match data_type {
            DataType::Static(static_item) => Ok(DescriptorEntryBuilder {
                key: static_item.key.clone(),
                expression: Expression::new(format!("'{}'", static_item.value).as_str())?,
            }),
            DataType::Expression(exp_item) => Ok(DescriptorEntryBuilder {
                key: exp_item.key.clone(),
                expression: Expression::new(&exp_item.value)?,
            }),
        }
    }

    pub fn evaluate(&self) -> Result<RateLimitDescriptor_Entry, EvaluationError> {
        let key = self.key.clone();
        let value = match self.expression.eval() {
            Ok(value) => match value {
                Value::Int(n) => format!("{n}"),
                Value::UInt(n) => format!("{n}"),
                Value::Float(n) => format!("{n}"),
                Value::String(s) => (*s).clone(),
                Value::Bool(b) => format!("{b}"),
                Value::Null => "null".to_owned(),
                _ => {
                    error!(
                        "Failed to match type for expression `{:?}`",
                        self.expression
                    );
                    return Err(EvaluationError::new(
                        self.expression.clone(),
                        "Only scalar values can be sent as data".to_string(),
                    ));
                }
            },
            Err(err) => {
                error!("Failed to evaluate `{:?}`: {err}", self.expression);
                return Err(EvaluationError::new(
                    self.expression.clone(),
                    format!("Evaluation failed: {err}"),
                ));
            }
        };
        let mut descriptor_entry = RateLimitDescriptor_Entry::new();
        descriptor_entry.set_key(key);
        descriptor_entry.set_value(value);
        Ok(descriptor_entry)
    }
}

#[derive(Debug, PartialEq, Clone)]
struct ConditionalData {
    pub data: Vec<DescriptorEntryBuilder>,
    pub predicates: Vec<Predicate>,
}

impl ConditionalData {
    pub fn new(action: &Action) -> Result<Self, ParseError> {
        let mut predicates = Vec::default();
        for predicate in &action.predicates {
            predicates.push(Predicate::new(predicate)?);
        }

        let mut data = Vec::default();
        for datum in &action.data {
            data.push(DescriptorEntryBuilder::new(&datum.item)?);
        }
        Ok(ConditionalData { data, predicates })
    }

    fn predicates_apply(&self) -> PredicateResult {
        if self.predicates.is_empty() {
            return Ok(true);
        }
        for predicate in &self.predicates {
            // if it does not apply or errors exit early
            if !predicate.test()? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub fn entries(&self) -> Result<RepeatedField<RateLimitDescriptor_Entry>, EvaluationError> {
        if !self.predicates_apply()? {
            return Ok(RepeatedField::default());
        }

        let mut entries = RepeatedField::default();
        for entry_builder in self.data.iter() {
            if !KNOWN_ATTRIBUTES.contains(&entry_builder.key.as_str()) {
                entries.push(entry_builder.evaluate()?);
            }
        }

        Ok(entries)
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct RateLimitAction {
    grpc_service: Rc<GrpcService>,
    scope: String,
    service_name: String,
    conditional_data_sets: Vec<ConditionalData>,
}

impl RateLimitAction {
    pub fn new(action: &Action, service: &Service) -> Result<Self, ParseError> {
        Ok(Self {
            grpc_service: Rc::new(GrpcService::new(Rc::new(service.clone()))),
            scope: action.scope.clone(),
            service_name: action.service.clone(),
            conditional_data_sets: vec![ConditionalData::new(action)?],
        })
    }

    pub fn build_descriptor(&self) -> Result<RateLimitDescriptor, EvaluationError> {
        let mut entries = RepeatedField::default();

        for conditional_data in self.conditional_data_sets.iter() {
            entries.extend(conditional_data.entries()?);
        }

        let mut res = RateLimitDescriptor::new();
        res.set_entries(entries);
        Ok(res)
    }

    pub fn get_known_attributes(&self) -> Result<(u32, String), EvaluationError> {
        let mut hits_addend = 1;
        let mut domain = String::new();

        for conditional_data in &self.conditional_data_sets {
            for entry_builder in &conditional_data.data {
                if KNOWN_ATTRIBUTES.contains(&entry_builder.key.as_str()) {
                    let val = entry_builder.expression.eval().map_err(|err| {
                        EvaluationError::new(
                            entry_builder.expression.clone(),
                            format!("Failed to evaluate expression: {err}"),
                        )
                    })?;
                    match entry_builder.key.as_str() {
                        "ratelimit.domain" => match val {
                            Value::String(s) => {
                                if s.is_empty() {
                                    return Err(EvaluationError::new(
                                        entry_builder.expression.clone(),
                                        "ratelimit.domain cannot be empty".to_string(),
                                    ));
                                }
                                domain = s.to_string();
                            }
                            _ => {
                                return Err(EvaluationError::new(
                                    entry_builder.expression.clone(),
                                    format!("Expected string for ratelimit.domain, got: {val:?}"),
                                ));
                            }
                        },
                        "ratelimit.hits_addend" => match val {
                            Value::Int(i) => {
                                if i >= 0 && i <= u32::MAX as i64 {
                                    hits_addend = i as u32;
                                } else {
                                    return Err(EvaluationError::new(entry_builder.expression.clone(), format!("ratelimit.hits_addend must be a non-negative integer, got: {val:?}")));
                                }
                            }
                            Value::UInt(u) => {
                                if u <= u32::MAX as u64 {
                                    hits_addend = u as u32;
                                } else {
                                    return Err(EvaluationError::new(entry_builder.expression.clone(), format!("ratelimit.hits_addend must be a non-negative integer, got: {val:?}")));
                                }
                            }
                            _ => {
                                return Err(EvaluationError::new(entry_builder.expression.clone(), format!("Only integer values are allowed for known attributes, got: {val:?}")));
                            }
                        },
                        _ => {}
                    }
                }
            }
        }
        Ok((hits_addend, domain))
    }

    pub fn get_grpcservice(&self) -> Rc<GrpcService> {
        Rc::clone(&self.grpc_service)
    }

    pub fn scope(&self) -> &str {
        self.scope.as_str()
    }

    pub fn conditions_apply(&self) -> PredicateResult {
        // For RateLimitAction conditions always apply.
        // It is when building the descriptor that it may be empty because predicates do not
        // evaluate to true.
        Ok(true)
    }

    pub fn get_failure_mode(&self) -> FailureMode {
        self.grpc_service.get_failure_mode()
    }

    pub fn resolve_failure_mode(&self) -> ResponseResult {
        match self.get_failure_mode() {
            FailureMode::Deny => Err(GrpcErrResponse::new_internal_server_error()),
            FailureMode::Allow => {
                debug!("process_response(rl): continuing as FailureMode Allow");
                Ok(HeaderKind::Response(Vec::default()))
            }
        }
    }

    pub fn validate_ratelimit_action_conditional_data(&self) -> Result<(), ActionCreationError> {
        for conditional_data_set in &self.conditional_data_sets {
            let mut seen_key_values: HashMap<String, String> = HashMap::new();

            for entry_builder in &conditional_data_set.data {
                let key = entry_builder.key.clone();
                let current_value = entry_builder.expression.to_string();

                if let Some(existing_value) = seen_key_values.get(&key) {
                    if *existing_value != current_value {
                        let error_message = format!(
                            "Invalid ConditionalDataSet: key '{key}' has conflicting internal values ('{existing_value}' and '{current_value}').",
                        );
                        error!("{error_message}");
                        return Err(ActionCreationError::InvalidAction(error_message));
                    }
                } else {
                    seen_key_values.insert(key, current_value);
                }
            }
        }
        Ok(())
    }

    pub fn merge(
        &mut self,
        other: RateLimitAction,
    ) -> Result<Option<RateLimitAction>, ActionCreationError> {
        self.validate_ratelimit_action_conditional_data()?;
        other.validate_ratelimit_action_conditional_data()?;

        if self.scope != other.scope || self.service_name != other.service_name {
            return Ok(Some(other));
        }

        let mut self_key_values: HashMap<String, String> = HashMap::new();
        for conditional_data_set in &self.conditional_data_sets {
            for entry_builder in &conditional_data_set.data {
                self_key_values.insert(
                    entry_builder.key.clone(),
                    entry_builder.expression.to_string(),
                );
            }
        }

        for other_conditional_data_set in &other.conditional_data_sets {
            for other_entry_builder in &other_conditional_data_set.data {
                let key = &other_entry_builder.key;
                let other_value = other_entry_builder.expression.to_string();

                if let Some(self_value) = self_key_values.get(key) {
                    if *self_value != other_value {
                        let error_message = format!(
                            "Conflicting values within RateLimitActions for key '{key}': '{self_value}' and '{other_value}'",
                        );
                        error!("{}", error_message);
                        return Err(ActionCreationError::InvalidAction(error_message));
                    }
                }
            }
        }

        self.conditional_data_sets
            .extend(other.conditional_data_sets);

        Ok(None)
    }

    pub fn process_response(
        &self,
        rate_limit_response: RateLimitResponse,
    ) -> Result<HeaderKind, GrpcErrResponse> {
        match rate_limit_response {
            RateLimitResponse {
                overall_code: RateLimitResponse_Code::UNKNOWN,
                ..
            } => {
                debug!("process_response(rl): received UNKNOWN response");
                self.resolve_failure_mode()
            }
            RateLimitResponse {
                overall_code: RateLimitResponse_Code::OVER_LIMIT,
                response_headers_to_add: rl_headers,
                ..
            } => {
                debug!("process_response(rl): received OVER_LIMIT response");
                let response_headers = Self::get_header_vec(rl_headers);
                Err(GrpcErrResponse::new(
                    StatusCode::TooManyRequests as u32,
                    response_headers,
                    "Too Many Requests\n".to_string(),
                ))
            }
            RateLimitResponse {
                overall_code: RateLimitResponse_Code::OK,
                response_headers_to_add: additional_headers,
                ..
            } => {
                debug!("process_response(rl): received OK response");
                Ok(HeaderKind::Response(Self::get_header_vec(
                    additional_headers,
                )))
            }
        }
    }

    fn get_header_vec(headers: RepeatedField<HeaderValue>) -> Headers {
        headers
            .iter()
            .map(|header| (header.key.to_owned(), header.value.to_owned()))
            .collect()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::configuration::{
        Action, DataItem, DataType, ExpressionItem, FailureMode, Service, ServiceType, StaticItem,
        Timeout,
    };
    use core::str;

    fn build_service() -> Service {
        build_service_with_failure_mode(FailureMode::default())
    }

    fn build_service_with_failure_mode(failure_mode: FailureMode) -> Service {
        Service {
            service_type: ServiceType::RateLimit,
            endpoint: "some_endpoint".into(),
            failure_mode,
            timeout: Timeout::default(),
        }
    }

    fn build_action(mut scope: String, predicates: Vec<String>, data: Vec<DataItem>) -> Action {
        if scope.is_empty() {
            scope = "some_scope".to_string();
        }

        Action {
            service: "some_service".into(),
            scope,
            predicates,
            data,
        }
    }

    fn build_ratelimit_response(
        status: RateLimitResponse_Code,
        headers: Option<Vec<(&str, &str)>>,
    ) -> RateLimitResponse {
        let mut response = RateLimitResponse::new();
        response.set_overall_code(status);
        match status {
            RateLimitResponse_Code::UNKNOWN => {}
            RateLimitResponse_Code::OVER_LIMIT | RateLimitResponse_Code::OK => {
                if let Some(header_list) = headers {
                    response.set_response_headers_to_add(build_headers(header_list))
                }
            }
        }
        response
    }

    fn build_headers(headers: Vec<(&str, &str)>) -> RepeatedField<HeaderValue> {
        headers
            .into_iter()
            .map(|(key, value)| {
                let mut hv = HeaderValue::new();
                hv.set_key(key.to_string());
                hv.set_value(value.to_string());
                hv
            })
            .collect::<RepeatedField<HeaderValue>>()
    }

    fn build_action_for_known_attribute(key: &str, value: &str) -> Action {
        let data = vec![DataItem {
            item: DataType::Expression(ExpressionItem {
                key: key.into(),
                value: value.into(),
            }),
        }];
        build_action(String::new(), vec!["true".into()], data)
    }

    #[test]
    fn empty_predicates_do_apply() {
        let action = build_action(String::new(), Vec::default(), Vec::default());
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service)
            .expect("action building failed. Maybe predicates compilation?");
        assert_eq!(rl_action.conditions_apply(), Ok(true));
    }

    #[test]
    fn even_with_falsy_predicates_conditions_apply() {
        let action = build_action(String::new(), vec!["false".into()], Vec::default());
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service)
            .expect("action building failed. Maybe predicates compilation?");
        assert_eq!(rl_action.conditions_apply(), Ok(true));
    }

    #[test]
    fn empty_data_generates_empty_descriptor() {
        let action = build_action(String::new(), Vec::default(), Vec::default());
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service)
            .expect("action building failed. Maybe predicates compilation?");
        assert_eq!(
            rl_action.build_descriptor(),
            Ok(RateLimitDescriptor::default())
        );
    }

    #[test]
    fn get_known_attribute_fails_on_empty_domain() {
        let action = build_action_for_known_attribute("ratelimit.domain", "''");
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service).unwrap();

        let result = rl_action.get_known_attributes();
        assert!(result.is_err());
        // Assuming EvaluationError implements Debug or Display
        let error_message = format!("{:?}", result.unwrap_err());
        assert!(error_message.contains("ratelimit.domain cannot be empty"));
    }

    #[test]
    fn get_known_attribute_fails_on_non_string_domain() {
        let action = build_action_for_known_attribute("ratelimit.domain", "123");
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service).unwrap();

        let result = rl_action.get_known_attributes();
        assert!(result.is_err());
        let error_message = format!("{:?}", result.unwrap_err());
        assert!(error_message.contains("Expected string for ratelimit.domain"));
        assert!(error_message.contains("got: Int(123)"));
    }

    #[test]
    fn get_known_attribute_fails_on_negative_hits_addend() {
        let action = build_action_for_known_attribute("ratelimit.hits_addend", "-1");
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service).unwrap();

        let result = rl_action.get_known_attributes();
        assert!(result.is_err());
        let error_message = format!("{:?}", result.unwrap_err());
        assert!(error_message.contains("ratelimit.hits_addend must be a non-negative integer"));
        assert!(error_message.contains("got: Int(-1)"));
    }

    #[test]
    fn get_known_attribute_fails_on_too_large_hits_addend() {
        let too_large_value = (u32::MAX as u64 + 1).to_string();
        let action = build_action_for_known_attribute("ratelimit.hits_addend", &too_large_value);
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service).unwrap();

        let result = rl_action.get_known_attributes();
        assert!(result.is_err());
        let error_message = format!("{:?}", result.unwrap_err());

        assert!(error_message.contains("ratelimit.hits_addend must be a non-negative integer"));
        assert!(error_message.contains(&format!("got: Int({too_large_value})")));
    }

    #[test]
    fn get_known_attribute_fails_on_non_integer_hits_addend() {
        let action = build_action_for_known_attribute("ratelimit.hits_addend", "'not-a-number'");
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service).unwrap();

        let result = rl_action.get_known_attributes();
        assert!(result.is_err());
        let error_message = format!("{:?}", result.unwrap_err());
        assert!(error_message.contains("Only integer values are allowed for known attributes"));
        assert!(error_message.contains("got: String"));
    }

    #[test]
    fn get_known_attributes_from_descriptor_entries() {
        let data = vec![
            DataItem {
                item: DataType::Expression(ExpressionItem {
                    key: "ratelimit.domain".into(),
                    value: "'test'".into(),
                }),
            },
            DataItem {
                item: DataType::Expression(ExpressionItem {
                    key: "ratelimit.hits_addend".into(),
                    value: "3".into(),
                }),
            },
        ];
        let action = build_action(String::new(), Vec::default(), data);
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service)
            .expect("action building failed. Maybe predicates compilation?");
        let descriptor = rl_action.build_descriptor().expect("is ok");
        let known_attributes = rl_action.get_known_attributes().expect("is ok");
        assert_eq!(descriptor.get_entries().len(), 0);
        let (hits_addend, domain) = known_attributes;
        assert_eq!(hits_addend, 3);
        assert_eq!(domain, "test");
    }

    #[test]
    fn descriptor_entry_from_expression() {
        let data = vec![
            DataItem {
                item: DataType::Expression(ExpressionItem {
                    key: "ratelimit.domain".into(),
                    value: "'test'".into(),
                }),
            },
            DataItem {
                item: DataType::Expression(ExpressionItem {
                    key: "ratelimit.hits_addend".into(),
                    value: "'3'".into(),
                }),
            },
            DataItem {
                item: DataType::Expression(ExpressionItem {
                    key: "key_1".into(),
                    value: "'value_1'".into(),
                }),
            },
        ];
        let action = build_action(String::new(), Vec::default(), data);
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service)
            .expect("action building failed. Maybe predicates compilation?");
        let descriptor = rl_action.build_descriptor().expect("is ok");
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
        let action = build_action(String::new(), Vec::default(), data);
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service)
            .expect("action building failed. Maybe predicates compilation?");
        let descriptor = rl_action.build_descriptor().expect("is ok");
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
        let action = build_action(String::new(), predicates, data);
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service)
            .expect("action building failed. Maybe predicates compilation?");
        let descriptor = rl_action.build_descriptor().expect("is ok");
        assert_eq!(descriptor, RateLimitDescriptor::default());
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
        let action_1 = build_action(String::new(), predicates_1, data_1);
        let mut rl_action_1 = RateLimitAction::new(&action_1, &service)
            .expect("action building failed. Maybe predicates compilation?");

        let data_2 = vec![DataItem {
            item: DataType::Expression(ExpressionItem {
                key: "key_2".into(),
                value: "'value_2'".into(),
            }),
        }];
        let predicates_2 = vec!["false".into()];
        let action_2 = build_action(String::new(), predicates_2, data_2);
        let rl_action_2 = RateLimitAction::new(&action_2, &service)
            .expect("action building failed. Maybe predicates compilation?");

        let data_3 = vec![DataItem {
            item: DataType::Expression(ExpressionItem {
                key: "key_3".into(),
                value: "'value_3'".into(),
            }),
        }];
        let predicates_3 = vec!["true".into()];
        let action_3 = build_action(String::new(), predicates_3, data_3);
        let rl_action_3 = RateLimitAction::new(&action_3, &service)
            .expect("action building failed. Maybe predicates compilation?");
        assert_eq!(rl_action_1.merge(rl_action_2), Ok(None));
        assert_eq!(rl_action_1.merge(rl_action_3), Ok(None));

        // it should generate descriptor entries from action 1 and action 3
        let descriptor = rl_action_1.build_descriptor().expect("is ok");
        assert_eq!(descriptor.get_entries().len(), 2);
        assert_eq!(descriptor.get_entries()[0].key, String::from("key_1"));
        assert_eq!(descriptor.get_entries()[0].value, String::from("value_1"));
        assert_eq!(descriptor.get_entries()[1].key, String::from("key_3"));
        assert_eq!(descriptor.get_entries()[1].value, String::from("value_3"));
    }

    #[test]
    fn merge_fails_on_conflicting_values_with_same_scope() {
        let service = build_service();

        let data_1 = vec![DataItem {
            item: DataType::Expression(ExpressionItem {
                key: "key_1".into(),
                value: "1".into(),
            }),
        }];
        let predicates_1 = vec!["true".into()];
        let action_1 = build_action(String::new(), predicates_1, data_1);
        let mut rl_action_1 =
            RateLimitAction::new(&action_1, &service).expect("action building failed");

        let data_2 = vec![DataItem {
            item: DataType::Expression(ExpressionItem {
                key: "key_1".into(),
                value: "2".into(),
            }),
        }];
        let predicates_2 = vec!["false".into()];
        let action_2 = build_action(String::new(), predicates_2, data_2);
        let rl_action_2 =
            RateLimitAction::new(&action_2, &service).expect("action building failed");

        assert!(rl_action_1.merge(rl_action_2).is_err());
    }

    #[test]
    fn merge_fails_if_other_action_has_internal_conflicts() {
        let service = build_service();

        let data_1 = vec![DataItem {
            item: DataType::Expression(ExpressionItem {
                key: "key_1".into(),
                value: "1".into(),
            }),
        }];
        let predicates_1 = vec!["true".into()];
        let action_1 = build_action(String::new(), predicates_1, data_1);
        let mut rl_action_1 =
            RateLimitAction::new(&action_1, &service).expect("action building failed");

        let data_3 = vec![
            DataItem {
                item: DataType::Expression(ExpressionItem {
                    key: "key_1".into(),
                    value: "3".into(),
                }),
            },
            DataItem {
                item: DataType::Expression(ExpressionItem {
                    key: "key_1".into(),
                    value: "2".into(),
                }),
            },
        ];
        let predicates_3 = vec!["true".into()];
        let action_3 = build_action("scope_3".to_string(), predicates_3, data_3);
        let rl_action_3 =
            RateLimitAction::new(&action_3, &service).expect("action building failed");

        assert!(rl_action_1.merge(rl_action_3).is_err());
    }

    #[test]
    fn merge_succeeds_with_identical_duplicate_keys() {
        let service = build_service();

        let data_1 = vec![DataItem {
            item: DataType::Expression(ExpressionItem {
                key: "key_1".into(),
                value: "1".into(),
            }),
        }];
        let predicates_1 = vec!["true".into()];
        let action_1 = build_action(String::new(), predicates_1, data_1);
        let mut rl_action_1 =
            RateLimitAction::new(&action_1, &service).expect("action building failed");

        let data_4 = vec![
            DataItem {
                item: DataType::Expression(ExpressionItem {
                    key: "key_1".into(),
                    value: "1".into(),
                }),
            },
            DataItem {
                item: DataType::Expression(ExpressionItem {
                    key: "key_1".into(),
                    value: "1".into(),
                }),
            },
        ];
        let predicates_4 = vec!["true".into()];
        let action_4 = build_action(String::new(), predicates_4, data_4);
        let rl_action_4 =
            RateLimitAction::new(&action_4, &service).expect("action building failed");

        assert_eq!(rl_action_1.merge(rl_action_4), Ok(None));
        let (hits_addend, domain) = rl_action_1.get_known_attributes().expect("is ok");
        assert_eq!(hits_addend, 1);
        assert_eq!(domain, "");
    }

    #[test]
    fn process_ok_response() {
        let action = build_action(String::new(), Vec::default(), Vec::default());
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service)
            .expect("action building failed. Maybe predicates compilation?");

        let ok_response_without_headers =
            build_ratelimit_response(RateLimitResponse_Code::OK, None);
        let result = rl_action.process_response(ok_response_without_headers);
        assert!(result.is_ok());

        let headers = result.expect("is ok");
        assert!(headers.is_empty());

        let ok_response_with_header = build_ratelimit_response(
            RateLimitResponse_Code::OK,
            Some(vec![("my_header", "my_value")]),
        );
        let result = rl_action.process_response(ok_response_with_header);
        assert!(result.is_ok());

        match result.expect("is ok") {
            HeaderKind::Response(headers) => {
                assert!(!headers.is_empty());
                assert_eq!(
                    headers[0],
                    ("my_header".to_string(), "my_value".to_string())
                );
            }
            HeaderKind::Request(_headers) => {
                unreachable!("ratelimitresponse should not return Request headers")
            }
        }
    }

    #[test]
    fn process_overlimit_response() {
        let headers = vec![("x-ratelimit-limit", "10"), ("x-ratelimit-remaining", "0")];
        let action = build_action(String::new(), Vec::default(), Vec::default());
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service)
            .expect("action building failed. Maybe predicates compilation?");

        let overlimit_response_empty =
            build_ratelimit_response(RateLimitResponse_Code::OVER_LIMIT, None);
        let result = rl_action.process_response(overlimit_response_empty);
        assert!(result.is_err());

        let grpc_err_response = result.expect_err("is err");
        assert_eq!(
            grpc_err_response.status_code(),
            StatusCode::TooManyRequests as u32
        );
        assert!(grpc_err_response.headers().is_empty());
        assert_eq!(grpc_err_response.body(), "Too Many Requests\n");

        let denied_response_headers =
            build_ratelimit_response(RateLimitResponse_Code::OVER_LIMIT, Some(headers.clone()));
        let result = rl_action.process_response(denied_response_headers);
        assert!(result.is_err());

        let grpc_err_response = result.expect_err("is err");
        assert_eq!(
            grpc_err_response.status_code(),
            StatusCode::TooManyRequests as u32
        );

        let response_headers = grpc_err_response.headers();
        headers.iter().zip(response_headers.iter()).for_each(
            |((header_one, value_one), (header_two, value_two))| {
                assert_eq!(header_one, header_two);
                assert_eq!(value_one, value_two);
            },
        );

        assert_eq!(grpc_err_response.body(), "Too Many Requests\n");
    }

    #[test]
    fn process_error_response() {
        let action = build_action(String::new(), Vec::default(), Vec::default());
        let deny_service = build_service_with_failure_mode(FailureMode::Deny);
        let rl_action = RateLimitAction::new(&action, &deny_service)
            .expect("action building failed. Maybe predicates compilation?");

        let error_response = build_ratelimit_response(RateLimitResponse_Code::UNKNOWN, None);
        let result = rl_action.process_response(error_response.clone());
        assert!(result.is_err());

        let grpc_err_response = result.expect_err("is err");
        assert_eq!(
            grpc_err_response.status_code(),
            StatusCode::InternalServerError as u32
        );

        assert!(grpc_err_response.headers().is_empty());
        assert_eq!(grpc_err_response.body(), "Internal Server Error.\n");

        let allow_service = build_service_with_failure_mode(FailureMode::Allow);
        let rl_action = RateLimitAction::new(&action, &allow_service)
            .expect("action building failed. Maybe predicates compilation?");

        let result = rl_action.process_response(error_response);
        assert!(result.is_ok());

        let headers = result.expect("is ok");
        assert!(headers.is_empty());
    }
}
