use crate::envoy::rate_limit_descriptor::Entry;
use crate::envoy::{rate_limit_response, RateLimitDescriptor};
use crate::v2::data::attribute::{AttributeError, AttributeState};
use crate::v2::data::cel::errors::EvaluationError;
use crate::v2::data::cel::{Expression, Predicate, PredicateVec};
use crate::v2::kuadrant::pipeline::blueprint::ConditionalData;
#[allow(dead_code)]
use crate::v2::kuadrant::pipeline::tasks::{PendingTask, Task, TaskOutcome};
use crate::v2::kuadrant::ReqRespCtx;
use crate::v2::services::{RateLimitService, Service};
use cel_interpreter::Value;
use log::error;
use std::rc::Rc;

/// Builds individual descriptor entries from CEL expressions
struct DescriptorEntryBuilder {
    key: String,
    expression: Expression,
}

impl DescriptorEntryBuilder {
    /// Create an instance of DescriptorEntryBuilder
    fn new(key: String, expression: Expression) -> Self {
        Self { key, expression }
    }

    /// Evaluate the expression to create a descriptor entry
    fn evaluate(self, ctx: &ReqRespCtx) -> Result<AttributeState<Entry>, EvaluationError> {
        match self.expression.eval(ctx) {
            Ok(AttributeState::Available(value)) => {
                let value_str = match value {
                    Value::Int(n) => format!("{n}"),
                    Value::UInt(n) => format!("{n}"),
                    Value::Float(n) => format!("{n}"),
                    Value::String(s) => s.to_string(),
                    Value::Bool(b) => format!("{b}"),
                    Value::Null => "null".to_owned(),
                    _ => {
                        return Err(EvaluationError::new(
                            self.expression,
                            "Only scalar values can be sent as data".to_string(),
                        ));
                    }
                };

                Ok(AttributeState::Available(Entry {
                    key: self.key.clone(),
                    value: value_str,
                }))
            }
            Ok(AttributeState::Pending) => Ok(AttributeState::Pending),
            Err(cel_err) => Err(EvaluationError::new(
                self.expression,
                format!("CEL evaluation error for '{}': {}", self.key, cel_err),
            )),
        }
    }
}

const KNOWN_ATTRIBUTES: [&str; 2] = ["ratelimit.domain", "ratelimit.hits_addend"];

/// A task that performs rate limiting by sending descriptors to a rate limit service
pub struct RateLimitTask {
    task_id: String,
    dependencies: Vec<String>,
    pauses_filter: bool,

    // Rate limit configuration
    scope: String,
    service: Rc<RateLimitService>,

    // Conditional data for building descriptors
    conditional_data_sets: Vec<ConditionalData>,
    predicates: Vec<Predicate>,

    // Default hits addend
    default_hits_addend: u32,
}

/// Creates a new RL task
#[allow(dead_code)]
impl RateLimitTask {
    pub fn new(
        task_id: String,
        dependencies: Vec<String>,
        service: Rc<RateLimitService>,
        scope: String,
        predicates: Vec<Predicate>,
        conditional_data_sets: Vec<ConditionalData>,
        pauses_filter: bool,
    ) -> Self {
        Self {
            task_id,
            dependencies,
            pauses_filter,
            scope,
            service,
            predicates,
            conditional_data_sets,
            default_hits_addend: 1,
        }
    }

    /// Creates a new RL task prior caching its needed attributes
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_caching_attributes(
        ctx: &mut ReqRespCtx,
        task_id: String,
        service: Rc<RateLimitService>,
        scope: String,
        dependencies: Vec<String>,
        predicates: Vec<Predicate>,
        conditional_data_sets: Vec<ConditionalData>,
        pauses_filter: bool,
    ) -> Self {
        // Warming up the cache
        let _ = predicates.apply(ctx);
        let _ = conditional_data_sets.iter().map(|conditional_data| {
            let _ = conditional_data.predicates.apply(ctx);
            conditional_data
                .data
                .iter()
                .map(|data| data.value.eval(ctx))
        });

        Self::new(
            task_id,
            dependencies,
            service,
            scope,
            predicates,
            conditional_data_sets,
            pauses_filter,
        )
    }

    /// Builds the rate limit descriptors from the context
    fn build_descriptors(
        &self,
        ctx: &ReqRespCtx,
    ) -> Result<AttributeState<Vec<RateLimitDescriptor>>, EvaluationError> {
        let mut entries: Vec<Entry> = Vec::new();
        for conditional_data in &self.conditional_data_sets {
            match conditional_data.predicates.apply(ctx)? {
                AttributeState::Pending => return Ok(AttributeState::Pending),
                AttributeState::Available(false) => continue,
                AttributeState::Available(true) => {
                    for data_item in &conditional_data.data {
                        let entry_state = DescriptorEntryBuilder::new(
                            data_item.key.clone(),
                            data_item.value.clone(),
                        )
                        .evaluate(ctx)?;
                        match entry_state {
                            AttributeState::Available(entry) => {
                                if !KNOWN_ATTRIBUTES.contains(&entry.key.as_str()) {
                                    entries.push(entry)
                                }
                            }
                            AttributeState::Pending => return Ok(AttributeState::Pending),
                        }
                    }
                }
            }
        }

        if !entries.is_empty() {
            return Ok(AttributeState::Available(vec![RateLimitDescriptor {
                entries,
                limit: None,
            }]));
        }
        Ok(AttributeState::Available(Vec::new()))
    }

    /// Extract known attributes like ratelimit.domain and ratelimit.hits_addend
    fn get_known_attributes(&self, ctx: &ReqRespCtx) -> Result<(u32, String), AttributeError> {
        let mut hits_addend = self.default_hits_addend;
        let mut domain = String::new();

        for conditional_data in &self.conditional_data_sets {
            for data_item in &conditional_data.data {
                if KNOWN_ATTRIBUTES.contains(&data_item.key.as_str()) {
                    match data_item.value.eval(ctx) {
                        Ok(AttributeState::Available(val)) => match data_item.key.as_str() {
                            "ratelimit.domain" => {
                                if let Value::String(s) = val {
                                    if s.is_empty() {
                                        return Err(AttributeError::Parse(
                                            "ratelimit.domain cannot be empty".to_string(),
                                        ));
                                    }
                                    domain = s.to_string();
                                } else {
                                    return Err(AttributeError::Parse(
                                        "ratelimit.domain must be string".to_string(),
                                    ));
                                }
                            }
                            "ratelimit.hits_addend" => match val {
                                Value::Int(i) if i >= 0 && i <= u32::MAX as i64 => {
                                    hits_addend = i as u32;
                                }
                                Value::UInt(u) if u <= u32::MAX as u64 => {
                                    hits_addend = u as u32;
                                }
                                _ => {
                                    return Err(AttributeError::Parse(
                                        "ratelimit.hits_addend must be 0 <= X <= u32::MAX"
                                            .to_string(),
                                    ));
                                }
                            },
                            _ => {}
                        },
                        Ok(AttributeState::Pending) => {
                            return Err(AttributeError::NotAvailable(format!(
                                "Attribute {} is pending",
                                data_item.key
                            )));
                        }
                        Err(cel_err) => {
                            return Err(AttributeError::Retrieval(format!(
                                "CEL evaluation error: {}",
                                cel_err
                            )));
                        }
                    }
                }
            }
        }
        Ok((hits_addend, domain))
    }
}

impl Task for RateLimitTask {
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        match self.predicates.apply(ctx) {
            Ok(AttributeState::Pending) => return TaskOutcome::Requeued(vec![self]),
            Ok(AttributeState::Available(false)) => return TaskOutcome::Done,
            Ok(AttributeState::Available(true)) => {}
            Err(e) => {
                error!("Failed to apply predicates: {e:?}");
                return TaskOutcome::Failed;
            }
        }

        // Build the rate limit descriptors
        let descriptors = match self.build_descriptors(ctx) {
            Ok(AttributeState::Available(descriptors)) => descriptors,
            Ok(AttributeState::Pending) => {
                // Need to wait for attributes, requeue
                return TaskOutcome::Requeued(vec![self]);
            }
            Err(_e) => {
                // TODO: Handle error appropriately based on failure mode
                return TaskOutcome::Failed;
            }
        };
        // Extract known attributes (hits_addend, domain) before filtering
        let (hits_addend, domain_override) = match self.get_known_attributes(ctx) {
            Ok(attrs) => attrs,
            Err(_err) => return TaskOutcome::Failed, // should we fail or requeue?
        };

        // Determine domain (use override or default scope)
        let domain: String = if domain_override.is_empty() {
            self.scope.clone()
        } else {
            domain_override
        };

        // If empty message, skip rate limiting... Or should it be TaskOutcome::Failed?
        if descriptors.is_empty() {
            return TaskOutcome::Done;
        }

        // Dispatch the rate limit service message
        let token_id = match self
            .service
            .dispatch_ratelimit(ctx, &domain, descriptors, hits_addend)
        {
            Ok(id) => id,
            Err(_e) => {
                // TODO: Handle error based on failure mode (allow/deny)
                return TaskOutcome::Failed;
            }
        };

        // Prepare response processing
        let service = self.service.clone();

        // Return deferred outcome with response processor
        TaskOutcome::Deferred {
            token_id,
            pending: PendingTask {
                task_id: Some(self.task_id),
                pauses_filter: self.pauses_filter,
                process_response: Box::new(move |ctx, _status_code, response_size| {
                    let rate_limit_response = match service.get_response(ctx, response_size) {
                        Ok(parsed) => parsed,
                        Err(_e) => {
                            // TODO: Handle parsing error
                            return TaskOutcome::Failed;
                        }
                    };

                    // Process based on response code
                    match rate_limit_response.overall_code {
                        code if code == rate_limit_response::Code::Ok as i32 => {
                            // Rate limit check passed
                            // TODO: Extract headers and push ModifyHeadersTask + DirectResponseTask
                            todo!()
                        }
                        code if code == rate_limit_response::Code::OverLimit as i32 => {
                            // Rate limit exceeded - return 429
                            // TODO: Extract headers and push ModifyHeadersTask + DirectResponseTask
                            todo!()
                        }
                        _ => {
                            // Unknown or error response
                            // TODO: Handle parsing error and/or FailureMode task?
                            todo!()
                        }
                    }
                }),
            },
        }
    }

    fn id(&self) -> Option<String> {
        Some(self.task_id.clone())
    }

    fn dependencies(&self) -> &[String] {
        self.dependencies.as_slice()
    }

    fn pauses_filter(&self, _ctx: &ReqRespCtx) -> bool {
        self.pauses_filter
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::data::cel::Expression;
    use crate::v2::data::cel::Predicate;
    use crate::v2::kuadrant::pipeline::blueprint::DataItem;
    use crate::v2::kuadrant::resolver::MockWasmHost;
    use crate::v2::kuadrant::ReqRespCtx;
    use std::sync::Arc;

    fn create_test_context() -> ReqRespCtx {
        let mock_host = MockWasmHost::new();
        ReqRespCtx::new(Arc::new(mock_host))
    }

    fn create_test_context_with_headers(headers: Vec<(String, String)>) -> ReqRespCtx {
        let mock_host = MockWasmHost::new().with_map("request.headers".to_string(), headers);
        ReqRespCtx::new(Arc::new(mock_host))
    }

    fn create_test_service() -> RateLimitService {
        RateLimitService::new(
            "test".to_string(),
            std::time::Duration::from_secs(1),
            "test",
            "POST",
        )
    }

    fn create_test_task_with(
        ctx: &mut ReqRespCtx,
        top_predicates: Vec<Predicate>,
        conditional_data: Vec<ConditionalData>,
    ) -> RateLimitTask {
        RateLimitTask::new_with_caching_attributes(
            ctx,
            "test".to_string(),
            Rc::new(create_test_service()),
            "test".to_string(),
            vec![],
            top_predicates,
            conditional_data,
            false,
        )
    }

    /*
    // TODO: Fix this test
    #[test]
    fn test_descriptor_builder_with_headers() {
        let mut ctx = create_test_context_with_headers(vec![
            ("host".to_string(), "example.com".to_string()),
        ]);
        let expression = Expression::new("request.headers.host").unwrap();
        let builder = DescriptorEntryBuilder::new("host_key".to_string(), expression);

        let result = builder.evaluate(&ctx).unwrap();

        assert_eq!(result.key, "host_key");
        assert_eq!(result.value, "example.com");
    } */

    #[test]
    fn test_default_values_when_no_known_attributes() {
        let mut ctx = create_test_context();
        let task = RateLimitTask::new_with_caching_attributes(
            &mut ctx,
            "test".to_string(),
            Rc::new(create_test_service()),
            "test".to_string(),
            vec![],
            vec![],
            vec![],
            false,
        );

        let (hits_addend, domain) = task.get_known_attributes(&ctx).unwrap();

        assert_eq!(hits_addend, 1);
        assert_eq!(domain, "");
    }

    #[test]
    fn test_default_values_with_known_attributes() {
        let mut ctx = create_test_context();
        let conditional_data = ConditionalData {
            predicates: vec![],
            data: vec![
                DataItem {
                    key: "ratelimit.domain".to_string(),
                    value: Expression::new("\"example.org\"").unwrap(),
                },
                DataItem {
                    key: "ratelimit.hits_addend".to_string(),
                    value: Expression::new("5").unwrap(),
                },
            ],
        };
        let task = RateLimitTask::new_with_caching_attributes(
            &mut ctx,
            "test".to_string(),
            Rc::new(create_test_service()),
            "test".to_string(),
            vec![],
            vec![],
            vec![conditional_data],
            false,
        );

        let (hits_addend, domain) = task.get_known_attributes(&ctx).unwrap();

        assert_eq!(hits_addend, 5);
        assert_eq!(domain, "example.org");
    }

    #[test]
    fn test_build_descriptors_filters_known_attributes() {
        let mut ctx = create_test_context();
        let conditional_data = ConditionalData {
            predicates: vec![],
            data: vec![
                DataItem {
                    key: "ratelimit.domain".to_string(),
                    value: Expression::new("\"my-domain\"").unwrap(),
                },
                DataItem {
                    key: "ratelimit.hits_addend".to_string(),
                    value: Expression::new("5").unwrap(),
                },
                DataItem {
                    key: "actual_key".to_string(),
                    value: Expression::new("\"actual_value\"").unwrap(),
                },
            ],
        };
        let task = RateLimitTask::new_with_caching_attributes(
            &mut ctx,
            "test".to_string(),
            Rc::new(create_test_service()),
            "test".to_string(),
            vec![],
            vec![],
            vec![conditional_data],
            false,
        );

        let result = task.build_descriptors(&ctx).unwrap();

        match result {
            AttributeState::Available(descriptors) => {
                assert_eq!(descriptors.len(), 1);
                // Known attributes should be filtered out
                assert_eq!(descriptors[0].entries.len(), 1);
                assert_eq!(descriptors[0].entries[0].key, "actual_key");
            }
            AttributeState::Pending => unreachable!("Expected Ready, got Pending"),
        }
    }

    #[test]
    fn test_build_descriptors_with_one_conditional_failing_predicate() {
        let mut ctx = create_test_context();
        let conditional_data = vec![
            ConditionalData {
                predicates: vec![
                    Predicate::new("true").unwrap(),
                    Predicate::new("false").unwrap(),
                ],
                data: vec![DataItem {
                    key: "test_key".to_string(),
                    value: Expression::new("\"test_value\"").unwrap(),
                }],
            },
            ConditionalData {
                predicates: vec![
                    Predicate::new("true").unwrap(),
                    Predicate::new("true").unwrap(),
                ],
                data: vec![DataItem {
                    key: "test_key2".to_string(),
                    value: Expression::new("\"test_value2\"").unwrap(),
                }],
            },
        ];
        let task = create_test_task_with(
            &mut ctx,
            vec![Predicate::new("true").unwrap()],
            conditional_data,
        );

        let result = task.build_descriptors(&ctx).unwrap();

        match result {
            AttributeState::Available(descriptors) => {
                // One of the Conditional Data Predicates failed, so sonly one descriptor should be built
                assert_eq!(descriptors.len(), 1);
                assert_eq!(descriptors[0].entries.len(), 1);
                assert_eq!(descriptors[0].entries[0].key, "test_key2");
            }
            AttributeState::Pending => unreachable!("Expected Ready, got Pending"),
        }
    }

    #[test]
    fn test_build_descriptors_with_passing_predicates() {
        let mut ctx = create_test_context();
        let conditional_data = vec![
            ConditionalData {
                predicates: vec![],
                data: vec![DataItem {
                    key: "key_1".to_string(),
                    value: Expression::new("42").unwrap(),
                }],
            },
            ConditionalData {
                predicates: vec![],
                data: vec![DataItem {
                    key: "key_2".to_string(),
                    value: Expression::new("420").unwrap(),
                }],
            },
        ];
        let task = create_test_task_with(
            &mut ctx,
            vec![Predicate::new("true").unwrap()],
            conditional_data,
        );

        let result = task.build_descriptors(&ctx).unwrap();

        match result {
            AttributeState::Available(descriptors) => {
                assert_eq!(descriptors.len(), 1);
                assert_eq!(descriptors[0].entries.len(), 2);
                assert_eq!(descriptors[0].entries[0].key, "key_1");
                assert_eq!(descriptors[0].entries[0].value, "42");
                assert_eq!(descriptors[0].entries[1].key, "key_2");
                assert_eq!(descriptors[0].entries[1].value, "420");
            }
            AttributeState::Pending => unreachable!("Expected Ready, got Pending"),
        }
    }

    #[test]
    fn test_build_descriptors_with_failing_conditional_data_predicate() {
        let mut ctx = create_test_context();
        let conditional_data = ConditionalData {
            predicates: vec![Predicate::new("false").unwrap()],
            data: vec![DataItem {
                key: "test_key".to_string(),
                value: Expression::new("\"test_value\"").unwrap(),
            }],
        };
        let task = create_test_task_with(&mut ctx, vec![], vec![conditional_data]);

        let result = task.build_descriptors(&ctx).unwrap();

        match result {
            AttributeState::Available(descriptors) => {
                assert_eq!(descriptors.len(), 0);
            }
            AttributeState::Pending => unreachable!("Expected Ready, got Pending"),
        }
    }

    #[test]
    fn test_build_descriptors_with_passing_conditional_data_predicate() {
        let mut ctx = create_test_context();
        let conditional_data = ConditionalData {
            predicates: vec![Predicate::new("true").unwrap()],
            data: vec![DataItem {
                key: "test_key".to_string(),
                value: Expression::new("\"test_value\"").unwrap(),
            }],
        };
        let task = create_test_task_with(&mut ctx, vec![], vec![conditional_data]);

        let result = task.build_descriptors(&ctx).unwrap();

        match result {
            AttributeState::Available(descriptors) => {
                assert_eq!(descriptors.len(), 1);
                assert_eq!(descriptors[0].entries.len(), 1);
            }
            AttributeState::Pending => unreachable!("Expected Ready, got Pending"),
        }
    }

    #[test]
    fn test_task_outcome_done() {
        let mut ctx = create_test_context();
        let task = Box::new(create_test_task_with(
            &mut ctx,
            vec![Predicate::new("false").unwrap()],
            vec![],
        ));

        let outcome = task.apply(&mut ctx);

        assert!(matches!(outcome, TaskOutcome::Done));
    }

    #[test]
    fn test_task_outcome_deferred() {
        let mut ctx = create_test_context();
        let conditional_data = ConditionalData {
            predicates: vec![],
            data: vec![DataItem {
                key: "test_key".to_string(),
                value: Expression::new("\"test_value\"").unwrap(),
            }],
        };
        let task = Box::new(create_test_task_with(
            &mut ctx,
            vec![Predicate::new("true").unwrap()],
            vec![conditional_data],
        ));

        let outcome = task.apply(&mut ctx);

        assert!(matches!(
            outcome,
            TaskOutcome::Deferred {
                token_id: _,
                pending: _
            }
        ));
    }

    // TODO: More specific testing for task outcomes when the rate limit response task is done
}
