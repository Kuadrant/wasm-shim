use crate::configuration::{Action, DataType, FailureMode, Service};
use crate::data::{Attribute, AttributeOwner, AttributeResolver, PredicateResult};
use crate::data::{Expression, Predicate};
use crate::envoy::{
    rate_limit_descriptor, rate_limit_response, RateLimitDescriptor, RateLimitResponse,
};
use crate::filter::operations::EventualOperation;
use crate::runtime_action::ResponseResult;
use crate::service::errors::{BuildMessageError, ProcessGrpcMessageError};
use crate::service::rate_limit::RateLimitService;
use crate::service::{from_envoy_rl_headers, DirectResponse, GrpcService};
use cel_interpreter::Value;
use cel_parser::ParseError;
use log::debug;

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

    pub fn evaluate<T>(
        &self,
        resolver: &mut T,
    ) -> Result<rate_limit_descriptor::Entry, BuildMessageError>
    where
        T: AttributeResolver,
    {
        let key = self.key.clone();
        let value = self.expression.eval(resolver)?;
        let value_str = match value {
            Value::Int(n) => format!("{n}"),
            Value::UInt(n) => format!("{n}"),
            Value::Float(n) => format!("{n}"),
            Value::String(s) => (*s).clone(),
            Value::Bool(b) => format!("{b}"),
            Value::Null => "null".to_owned(),
            _ => {
                return Err(BuildMessageError::new_unsupported_data_type_err(
                    self.expression.clone(),
                    value.type_of().to_string(),
                    "Only scalar values can be sent as data".to_string(),
                ));
            }
        };
        let descriptor_entry = rate_limit_descriptor::Entry {
            key,
            value: value_str,
        };
        Ok(descriptor_entry)
    }
}

#[derive(Debug, PartialEq, Clone)]
struct ConditionalData {
    pub data: Vec<DescriptorEntryBuilder>,
    pub predicates: Vec<Predicate>,
}

impl ConditionalData {
    pub fn new(config: &crate::configuration::ConditionalData) -> Result<Self, ParseError> {
        let mut predicates = Vec::default();
        for predicate in &config.predicates {
            predicates.push(Predicate::new(predicate)?);
        }

        let mut data = Vec::default();
        for datum in &config.data {
            data.push(DescriptorEntryBuilder::new(&datum.item)?);
        }
        Ok(ConditionalData { data, predicates })
    }

    fn predicates_apply<T>(&self, resolver: &mut T) -> PredicateResult
    where
        T: AttributeResolver,
    {
        if self.predicates.is_empty() {
            return Ok(true);
        }
        for predicate in &self.predicates {
            // if it does not apply or errors exit early
            if !predicate.test(resolver)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub fn entries<T>(
        &self,
        resolver: &mut T,
    ) -> Result<Vec<rate_limit_descriptor::Entry>, BuildMessageError>
    where
        T: AttributeResolver,
    {
        if !self.predicates_apply(resolver)? {
            return Ok(Vec::default());
        }

        let mut entries = Vec::default();
        for entry_builder in self.data.iter() {
            if !KNOWN_ATTRIBUTES.contains(&entry_builder.key.as_str()) {
                entries.push(entry_builder.evaluate(resolver)?);
            }
        }

        Ok(entries)
    }
}

impl AttributeOwner for ConditionalData {
    fn request_attributes(&self) -> Vec<&Attribute> {
        let mut attrs: Vec<&Attribute> = self
            .data
            .iter()
            .flat_map(|c| c.expression.request_attributes())
            .collect();
        attrs.extend(self.predicates.request_attributes().iter());
        attrs
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct RateLimitAction {
    grpc_service: Rc<GrpcService>,
    scope: String,
    service_name: String,
    conditional_data_sets: Vec<ConditionalData>,
    request_data: Vec<((String, String), Expression)>,
}

impl RateLimitAction {
    #[cfg(test)]
    pub fn new(action: &Action, service: &Service) -> Result<Self, ParseError> {
        Self::new_with_data(action, service, Vec::default())
    }

    pub fn new_with_data(
        action: &Action,
        service: &Service,
        request_data: Vec<((String, String), Expression)>,
    ) -> Result<Self, ParseError> {
        let conditional_data_sets = action
            .conditional_data
            .iter()
            .map(ConditionalData::new)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            grpc_service: Rc::new(GrpcService::new(Rc::new(service.clone()))),
            scope: action.scope.clone(),
            service_name: action.service.clone(),
            conditional_data_sets,
            request_data,
        })
    }

    pub fn build_message<T>(&self, resolver: &mut T) -> Result<Option<Vec<u8>>, BuildMessageError>
    where
        T: AttributeResolver,
    {
        let descriptor = self.build_descriptor(resolver)?;

        if descriptor.entries.is_empty() {
            debug!("build_message(rl): empty descriptors");
            Ok(None)
        } else {
            let (hits_addend, domain_attr) = self.get_known_attributes(resolver)?;
            let domain = if domain_attr.is_empty() {
                self.scope.clone()
            } else {
                domain_attr
            };
            let mut descriptors = vec![descriptor];
            if !self.request_data.is_empty() {
                let mut entries = Vec::default();
                for ((domain, field), expr) in self.request_data.iter() {
                    match expr.eval(resolver) {
                        Ok(Value::String(value)) => {
                            let key = if domain.is_empty() || domain == "metrics.labels" {
                                field.clone()
                            } else {
                                format!("{}.{}", domain, field)
                            };
                            let entry = rate_limit_descriptor::Entry {
                                key,
                                value: value.to_string(),
                            };
                            entries.push(entry);
                        }
                        Err(e) if e.is_transient() => {
                            //only interested in returning transient errors for so we can evaluate again later
                            debug!("build_message(rl): transient evaluation error for requestData field");
                            return Err(BuildMessageError::Evaluation(Box::new(e)));
                        }
                        _ => {}
                    }
                }

                let data = RateLimitDescriptor {
                    entries,
                    limit: None,
                };
                descriptors.push(data)
            }
            RateLimitService::request_message_as_bytes(domain, descriptors, hits_addend).map(Some)
        }
    }

    fn build_descriptor<T>(
        &self,
        resolver: &mut T,
    ) -> Result<RateLimitDescriptor, BuildMessageError>
    where
        T: AttributeResolver,
    {
        let mut entries = Vec::default();

        for conditional_data in self.conditional_data_sets.iter() {
            entries.extend(conditional_data.entries(resolver)?);
        }

        let res = RateLimitDescriptor {
            entries,
            limit: None,
        };
        Ok(res)
    }

    fn get_known_attributes<T>(&self, resolver: &mut T) -> Result<(u32, String), BuildMessageError>
    where
        T: AttributeResolver,
    {
        let mut hits_addend = 1;
        let mut domain = String::new();

        for conditional_data in &self.conditional_data_sets {
            for entry_builder in &conditional_data.data {
                if KNOWN_ATTRIBUTES.contains(&entry_builder.key.as_str()) {
                    let val = entry_builder.expression.eval(resolver)?;
                    match entry_builder.key.as_str() {
                        "ratelimit.domain" => match val {
                            Value::String(s) => {
                                if s.is_empty() {
                                    return Err(BuildMessageError::new_unsupported_data_type_err(
                                        entry_builder.expression.clone(),
                                        "empty string".to_string(),
                                        "ratelimit.domain cannot be empty".to_string(),
                                    ));
                                }
                                domain = s.to_string();
                            }
                            _ => {
                                return Err(BuildMessageError::new_unsupported_data_type_err(
                                    entry_builder.expression.clone(),
                                    val.type_of().to_string(),
                                    "string for ratelimit.domain".to_string(),
                                ));
                            }
                        },
                        "ratelimit.hits_addend" => match val {
                            Value::Int(i) => {
                                if i >= 0 && i <= u32::MAX as i64 {
                                    hits_addend = i as u32;
                                } else {
                                    return Err(BuildMessageError::new_unsupported_data_type_err(
                                        entry_builder.expression.clone(),
                                        i.to_string(),
                                        "ratelimit.hits_addend must be 0 <= X <= u32::MAX integer"
                                            .to_string(),
                                    ));
                                }
                            }
                            Value::UInt(u) => {
                                if u <= u32::MAX as u64 {
                                    hits_addend = u as u32;
                                } else {
                                    return Err(BuildMessageError::new_unsupported_data_type_err(
                                        entry_builder.expression.clone(),
                                        u.to_string(),
                                        "ratelimit.hits_addend must be 0 <= X <= u32::MAX integer"
                                            .to_string(),
                                    ));
                                }
                            }
                            _ => {
                                return Err(BuildMessageError::new_unsupported_data_type_err(
                                    entry_builder.expression.clone(),
                                    val.type_of().to_string(),
                                    "integer for ratelimit.hits_addend".to_string(),
                                ));
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

    pub fn conditions_apply(&self) -> PredicateResult {
        // For RateLimitAction conditions always apply.
        // It is when building the descriptor that it may be empty because predicates do not
        // evaluate to true.
        Ok(true)
    }

    pub fn get_failure_mode(&self) -> FailureMode {
        self.grpc_service.get_failure_mode()
    }

    pub fn process_response(&self, rate_limit_response: RateLimitResponse) -> ResponseResult {
        match rate_limit_response {
            RateLimitResponse {
                overall_code: code, ..
            } if code == rate_limit_response::Code::Unknown as i32 => {
                debug!("process_response(rl): received UNKNOWN response");
                Err(ProcessGrpcMessageError::UnsupportedField)
            }
            RateLimitResponse {
                overall_code: code,
                response_headers_to_add: rl_headers,
                ..
            } if code == rate_limit_response::Code::OverLimit as i32 => {
                debug!("process_response(rl): received OVER_LIMIT response");
                Ok(DirectResponse::new(
                    crate::envoy::StatusCode::TooManyRequests as u32,
                    from_envoy_rl_headers(rl_headers),
                    "Too Many Requests\n".to_string(),
                )
                .into())
            }
            RateLimitResponse {
                overall_code: code,
                response_headers_to_add: additional_headers,
                ..
            } if code == rate_limit_response::Code::Ok as i32 => {
                debug!("process_response(rl): received OK response");
                Ok(vec![EventualOperation::AddResponseHeaders(
                    from_envoy_rl_headers(additional_headers),
                )]
                .into())
            }
            _ => {
                debug!("process_response(rl): received unknown response code");
                Err(ProcessGrpcMessageError::UnsupportedField)
            }
        }
    }
}

impl AttributeOwner for RateLimitAction {
    fn request_attributes(&self) -> Vec<&Attribute> {
        self.conditional_data_sets
            .iter()
            .flat_map(|c| c.request_attributes())
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
    use crate::data::PathCache;
    use crate::envoy::HeaderValue;
    use crate::filter::operations::ProcessGrpcMessageOperation;
    use crate::service::errors::{BuildMessageError, ProcessGrpcMessageError};
    use cel_interpreter::objects::ValueType;
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
            predicates: Vec::default(),
            conditional_data: vec![crate::configuration::ConditionalData { predicates, data }],
        }
    }

    fn build_ratelimit_response(
        status: rate_limit_response::Code,
        headers: Option<Vec<(&str, &str)>>,
    ) -> RateLimitResponse {
        let response_headers_to_add = match status {
            rate_limit_response::Code::Unknown => Vec::new(),
            rate_limit_response::Code::OverLimit | rate_limit_response::Code::Ok => {
                headers.map(build_headers).unwrap_or_default()
            }
        };
        RateLimitResponse {
            overall_code: status as i32,
            response_headers_to_add,
            ..Default::default()
        }
    }

    fn build_headers(headers: Vec<(&str, &str)>) -> Vec<HeaderValue> {
        headers
            .into_iter()
            .map(|(key, value)| HeaderValue {
                key: key.to_string(),
                value: value.to_string(),
            })
            .collect()
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
        assert!(rl_action.conditions_apply().expect("should not fail!"));
    }

    #[test]
    fn even_with_falsy_predicates_conditions_apply() {
        let action = build_action(String::new(), vec!["false".into()], Vec::default());
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service)
            .expect("action building failed. Maybe predicates compilation?");
        assert!(rl_action.conditions_apply().expect("should not fail!"));
    }

    #[test]
    fn empty_data_generates_empty_descriptor() {
        let action = build_action(String::new(), Vec::default(), Vec::default());
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service)
            .expect("action building failed. Maybe predicates compilation?");
        assert_eq!(
            rl_action
                .build_descriptor(&mut PathCache::default())
                .expect("should not fail!"),
            RateLimitDescriptor::default()
        );
    }

    #[test]
    fn get_known_attribute_fails_on_empty_domain() {
        let action = build_action_for_known_attribute("ratelimit.domain", "''");
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service).unwrap();

        let result = rl_action.get_known_attributes(&mut PathCache::default());
        assert!(result.is_err());
        let error_message = format!("{:?}", result.unwrap_err());
        assert!(error_message.contains("ratelimit.domain cannot be empty"));
    }

    #[test]
    fn get_known_attribute_fails_on_non_string_domain() {
        let action = build_action_for_known_attribute("ratelimit.domain", "123");
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service).unwrap();

        let err = rl_action
            .get_known_attributes(&mut PathCache::default())
            .expect_err("should fail!");
        if let BuildMessageError::UnsupportedDataType {
            expression: _e,
            got,
            want,
        } = err
        {
            assert_eq!(got, "int");
            assert!(want.contains("string for ratelimit.domain"));
        } else {
            unreachable!("Not supposed to get here!");
        }
    }

    #[test]
    fn get_known_attribute_fails_on_negative_hits_addend() {
        let action = build_action_for_known_attribute("ratelimit.hits_addend", "-1");
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service).unwrap();

        let err = rl_action
            .get_known_attributes(&mut PathCache::default())
            .expect_err("should fail!");
        if let BuildMessageError::UnsupportedDataType {
            expression: _e,
            got,
            want,
        } = err
        {
            assert_eq!(got, "-1");
            assert!(want.contains("ratelimit.hits_addend must be"));
        } else {
            unreachable!("Not supposed to get here!");
        }
    }

    #[test]
    fn get_known_attribute_fails_on_too_large_hits_addend() {
        let too_large_value = (u32::MAX as u64 + 1).to_string();
        let action = build_action_for_known_attribute("ratelimit.hits_addend", &too_large_value);
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service).unwrap();

        let err = rl_action
            .get_known_attributes(&mut PathCache::default())
            .expect_err("should fail!");
        if let BuildMessageError::UnsupportedDataType {
            expression: _e,
            got,
            want,
        } = err
        {
            assert_eq!(got, too_large_value);
            assert!(want.contains("ratelimit.hits_addend must be"));
        } else {
            unreachable!("Not supposed to get here!");
        }
    }

    #[test]
    fn get_known_attribute_fails_on_non_integer_hits_addend() {
        let action = build_action_for_known_attribute("ratelimit.hits_addend", "'not-a-number'");
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service).unwrap();

        let err = rl_action
            .get_known_attributes(&mut PathCache::default())
            .expect_err("should fail!");
        if let BuildMessageError::UnsupportedDataType {
            expression: _e,
            got,
            want,
        } = err
        {
            assert_eq!(got, "string");
            assert!(want.contains("integer for ratelimit.hits_addend"));
        } else {
            unreachable!("Not supposed to get here!");
        }
    }

    #[test]
    fn get_known_attributes_from_descriptor_entries() {
        let mut resolver = PathCache::default();
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
        let descriptor = rl_action.build_descriptor(&mut resolver).expect("is ok");
        let known_attributes = rl_action
            .get_known_attributes(&mut resolver)
            .expect("is ok");
        assert_eq!(descriptor.entries.len(), 0);
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
        let descriptor = rl_action
            .build_descriptor(&mut PathCache::default())
            .expect("is ok");
        assert_eq!(descriptor.entries.len(), 1);
        assert_eq!(descriptor.entries[0].key, String::from("key_1"));
        assert_eq!(descriptor.entries[0].value, String::from("value_1"));
    }

    #[test]
    fn descriptor_entry_from_expression_evaluates_to_non_scalar() {
        let data = vec![DataItem {
            item: DataType::Expression(ExpressionItem {
                key: "foo".into(),
                value: "[]".into(),
            }),
        }];
        let action = build_action(String::new(), Vec::default(), data);
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service)
            .expect("action building failed. Maybe predicates compilation?");
        let build_err = rl_action
            .build_descriptor(&mut PathCache::default())
            .expect_err("is err");
        assert!(matches!(
            build_err,
            BuildMessageError::UnsupportedDataType { .. }
        ));
        if let BuildMessageError::UnsupportedDataType {
            expression: _,
            got,
            want: _,
        } = build_err
        {
            assert_eq!(got, ValueType::List.to_string());
        } else {
            unreachable!("Not supposed to get here!");
        }
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
        let descriptor = rl_action
            .build_descriptor(&mut PathCache::default())
            .expect("is ok");
        assert_eq!(descriptor.entries.len(), 1);
        assert_eq!(descriptor.entries[0].key, String::from("key_1"));
        assert_eq!(descriptor.entries[0].value, String::from("value_1"));
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
        let descriptor = rl_action
            .build_descriptor(&mut PathCache::default())
            .expect("is ok");
        assert_eq!(descriptor, RateLimitDescriptor::default());
    }

    #[test]
    fn process_ok_response() {
        let action = build_action(String::new(), Vec::default(), Vec::default());
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service)
            .expect("action building failed. Maybe predicates compilation?");

        let ok_response_without_headers =
            build_ratelimit_response(rate_limit_response::Code::Ok, None);
        let result = rl_action.process_response(ok_response_without_headers);
        assert!(result.is_ok());
        let op = result.expect("is ok");
        assert!(matches!(op, ProcessGrpcMessageOperation::EventualOps(_)));
        if let ProcessGrpcMessageOperation::EventualOps(ops) = op {
            assert_eq!(ops.len(), 1);
            assert!(matches!(ops[0], EventualOperation::AddResponseHeaders(_)));
            if let EventualOperation::AddResponseHeaders(headers) = &ops[0] {
                assert!(headers.is_empty());
            } else {
                unreachable!("Expected AddResponseHeaders operation");
            }
        } else {
            unreachable!("Expected EventualOps operation");
        }

        let ok_response_with_header = build_ratelimit_response(
            rate_limit_response::Code::Ok,
            Some(vec![("my_header", "my_value")]),
        );
        let result = rl_action.process_response(ok_response_with_header);
        assert!(result.is_ok());
        let op = result.expect("is ok");
        assert!(matches!(op, ProcessGrpcMessageOperation::EventualOps(_)));
        if let ProcessGrpcMessageOperation::EventualOps(ops) = op {
            assert_eq!(ops.len(), 1);
            assert!(matches!(ops[0], EventualOperation::AddResponseHeaders(_)));
            if let EventualOperation::AddResponseHeaders(headers) = &ops[0] {
                assert_eq!(headers.len(), 1);
                assert_eq!(
                    headers[0],
                    ("my_header".to_string(), "my_value".to_string())
                );
            } else {
                unreachable!("Expected AddResponseHeaders operation");
            }
        } else {
            unreachable!("Expected EventualOps operation");
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
            build_ratelimit_response(rate_limit_response::Code::OverLimit, None);
        let result = rl_action.process_response(overlimit_response_empty);
        assert!(result.is_ok());
        let op = result.expect("is ok");
        assert!(matches!(op, ProcessGrpcMessageOperation::DirectResponse(_)));
        if let ProcessGrpcMessageOperation::DirectResponse(direct_response) = op {
            assert_eq!(
                direct_response.status_code(),
                crate::envoy::envoy::r#type::v3::StatusCode::TooManyRequests as u32
            );
            assert!(direct_response.headers().is_empty());
            assert_eq!(direct_response.body(), "Too Many Requests\n");
        } else {
            unreachable!("Expected DirectResponse operation");
        }

        let denied_response_headers =
            build_ratelimit_response(rate_limit_response::Code::OverLimit, Some(headers.clone()));
        let result = rl_action.process_response(denied_response_headers);
        assert!(result.is_ok());
        let op = result.expect("is ok");
        assert!(matches!(op, ProcessGrpcMessageOperation::DirectResponse(_)));
        if let ProcessGrpcMessageOperation::DirectResponse(direct_response) = op {
            assert_eq!(
                direct_response.status_code(),
                crate::envoy::envoy::r#type::v3::StatusCode::TooManyRequests as u32
            );
            let response_headers = direct_response.headers();
            headers.iter().zip(response_headers.iter()).for_each(
                |((header_one, value_one), (header_two, value_two))| {
                    assert_eq!(header_one, header_two);
                    assert_eq!(value_one, value_two);
                },
            );
            assert_eq!(direct_response.body(), "Too Many Requests\n");
        } else {
            unreachable!("Expected DirectResponse operation");
        }
    }

    #[test]
    fn process_error_response() {
        let action = build_action(String::new(), Vec::default(), Vec::default());
        let deny_service = build_service_with_failure_mode(FailureMode::Deny);
        let rl_action = RateLimitAction::new(&action, &deny_service)
            .expect("action building failed. Maybe predicates compilation?");

        let error_response = build_ratelimit_response(rate_limit_response::Code::Unknown, None);
        let result = rl_action.process_response(error_response.clone());
        assert!(result.is_err());
        let err_response = result.expect_err("is err");
        assert!(matches!(
            err_response,
            ProcessGrpcMessageError::UnsupportedField
        ));
    }

    #[test]
    fn descriptor_entries_with_request_body_fails_unless_provided() {
        let data = vec![DataItem {
            item: DataType::Expression(ExpressionItem {
                key: "key_1".into(),
                value: "requestBodyJSON('/foo')".into(),
            }),
        }];

        let action = build_action(String::new(), vec![], data);
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service)
            .expect("action building failed. Maybe predicates compilation?");
        let err = rl_action
            .build_descriptor(&mut PathCache::default())
            .expect_err("should fail");
        if let BuildMessageError::Evaluation(e) = err {
            assert!(e.is_transient());
            assert_eq!(e.transient_property(), Some("request_body"));
        } else {
            unreachable!("Not supposed to get here!");
        }
    }

    #[test]
    fn ratelimit_action_request_attributes() {
        let data = vec![
            DataItem {
                item: DataType::Expression(ExpressionItem {
                    key: "key_1".into(),
                    value: "request.host".into(),
                }),
            },
            DataItem {
                item: DataType::Expression(ExpressionItem {
                    key: "key_2".into(),
                    value: "source.address".into(),
                }),
            },
        ];

        let predicates: Vec<String> = vec!["true".into(), "request.method == 'GET'".into()];

        let action = build_action(String::new(), predicates, data);
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service)
            .expect("action building failed. Maybe CEL compilation?");
        assert_eq!(rl_action.request_attributes().len(), 2);
    }

    #[test]
    fn build_message_does_not_eval_data_when_predicates_evaluate_to_false() {
        let data = vec![DataItem {
            item: DataType::Expression(ExpressionItem {
                key: "ratelimit.hits_addend".into(),
                // if the expression evaluates, it would fail as it's unknown
                value: "invalidFunc()".into(),
            }),
        }];
        let predicates = vec!["false".into()];

        let action = build_action("scope".into(), predicates, data);
        let service = build_service();

        let rl_action = RateLimitAction::new(&action, &service)
            .expect("action building failed. Maybe expression parsing?");

        let message = rl_action
            .build_message(&mut PathCache::default())
            .expect("this must not fail building");
        assert!(message.is_none());
    }

    #[test]
    fn test_build_message_uses_known_attributes() {
        let data = vec![
            DataItem {
                item: DataType::Expression(ExpressionItem {
                    key: "key_1".into(),
                    value: "'value_1'".into(),
                }),
            },
            DataItem {
                item: DataType::Expression(ExpressionItem {
                    key: "ratelimit.domain".into(),
                    value: "'test'".into(),
                }),
            },
            DataItem {
                item: DataType::Expression(ExpressionItem {
                    key: "ratelimit.hits_addend".into(),
                    value: "1+1".into(),
                }),
            },
        ];
        let predicates = vec![];
        let action = build_action("scope".into(), predicates, data);

        let service = build_service();

        let rl_action = RateLimitAction::new(&action, &service)
            .expect("action building failed. Maybe expression parsing?");

        let (hits_addend, domain) = rl_action
            .get_known_attributes(&mut PathCache::default())
            .unwrap();
        assert_eq!(hits_addend, 2);
        assert_eq!(domain, "test");
    }

    #[test]
    fn descriptor_entries_with_response_body_fails_unless_provided() {
        let data = vec![DataItem {
            item: DataType::Expression(ExpressionItem {
                key: "key_1".into(),
                value: "responseBodyJSON('/foo')".into(),
            }),
        }];

        let action = build_action(String::new(), vec![], data);
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service)
            .expect("action building failed. Maybe predicates compilation?");
        let err = rl_action
            .build_descriptor(&mut PathCache::default())
            .expect_err("should fail");
        if let BuildMessageError::Evaluation(e) = err {
            assert!(e.is_transient());
            assert_eq!(e.transient_property(), Some("response_body"));
        } else {
            unreachable!("Not supposed to get here!");
        }
    }

    #[test]
    fn build_message_with_request_body_and_response_body_in_known_attributes() {
        // simulate:
        // on request headers phase => no transient available -> fails
        // on request body phase => req body transients available -> fails (missing response body)
        // on response body phase => both transients available -> succeeds

        let mut resolver = PathCache::default();
        let data = vec![
            DataItem {
                item: DataType::Expression(ExpressionItem {
                    key: "model".into(),
                    value: "requestBodyJSON('/model')".into(),
                }),
            },
            DataItem {
                item: DataType::Expression(ExpressionItem {
                    key: "ratelimit.hits_addend".into(),
                    value: "responseBodyJSON('/usage/total_tokens')".into(),
                }),
            },
        ];

        let action = build_action(String::new(), vec![], data);
        let service = build_service();
        let rl_action = RateLimitAction::new(&action, &service)
            .expect("action building failed. Maybe predicates compilation?");
        let err = rl_action
            .build_message(&mut resolver)
            .expect_err("transients not available, it should fail!");
        if let BuildMessageError::Evaluation(e) = err {
            assert!(e.is_transient());
            assert_eq!(e.transient_property(), Some("request_body"));
        } else {
            unreachable!("Not supposed to get here!");
        }

        // request body phase
        let request_body = r#"
        {
            "model": "meta-llama/Llama-3.1-8B-Instruct",
            "input": "Tell me a three sentence story about a robot."
        }"#;
        resolver.add_transient("request_body", request_body.into());
        let err = rl_action
            .build_message(&mut resolver)
            .expect_err("resp body transient not available, it should fail!");
        if let BuildMessageError::Evaluation(e) = err {
            assert!(e.is_transient());
            assert_eq!(e.transient_property(), Some("response_body"));
        } else {
            unreachable!("Not supposed to get here!");
        }

        // response body phase
        let response_body = r#"
        {
          "id": "chatcmpl-2ee7427f-8b51-4f74-a5df-e6484df42547",
          "model": "meta-llama/Llama-3.1-8B-Instruct",
          "usage": {
            "prompt_tokens": 0,
            "completion_tokens": 11,
            "total_tokens": 11
          }
        }"#;
        resolver.add_transient("response_body", response_body.into());
        let message = rl_action.build_message(&mut resolver).expect("is ok");
        assert!(message.is_some());
    }
}
