use crate::envoy::{rate_limit_descriptor, rate_limit_response, RateLimitDescriptor};
use crate::v2::data::attribute::{AttributeError, AttributeState};
use crate::v2::data::cel::{Expression, Predicate, PredicateResult};
use crate::v2::kuadrant::pipeline::blueprint::{Action, ConditionalData};
#[allow(dead_code)]
use crate::v2::kuadrant::pipeline::tasks::{PendingTask, Task, TaskOutcome};
use crate::v2::kuadrant::ReqRespCtx;
use crate::v2::services::rate_limit::RateLimitService;
use crate::v2::services::Service;
use cel_interpreter::Value;
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
    fn evaluate(self, ctx: &ReqRespCtx) -> Result<rate_limit_descriptor::Entry, AttributeError> {
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
                        return Err(AttributeError::Parse(
                            "Only scalar values can be sent as data".to_string(),
                        ));
                    }
                };

                Ok(rate_limit_descriptor::Entry {
                    key: self.key.clone(),
                    value: value_str,
                })
            }
            Ok(AttributeState::Pending) => Err(AttributeError::NotAvailable(format!(
                "Expression '{}' evaluation is pending",
                self.key
            ))),
            Err(cel_err) => Err(AttributeError::Retrieval(format!(
                "CEL evaluation error for '{}': {}",
                self.key, cel_err
            ))),
        }
    }
}

/// Type indicating the state of the descriptors build outcome
enum BuildDescriptorsState {
    Ready(Vec<RateLimitDescriptor>),
    Pending,
}

const KNOWN_ATTRIBUTES: [&str; 2] = ["ratelimit.domain", "ratelimit.hits_addend"];

/// A task that performs rate limiting by sending descriptors to a rate limit service
pub struct RateLimitTask {
    task_id: String,
    dependencies: Vec<String>,
    is_blocking: bool,

    // Rate limit configuration
    scope: String,
    service: Rc<RateLimitService>,

    // Conditional data for building descriptors
    conditional_data_sets: Vec<ConditionalData>,
    predicates: Vec<Predicate>,

    // Default hits addend
    default_hits_addend: u32,
}

#[allow(dead_code)]
impl RateLimitTask {
    pub fn new(
        task_id: String,
        dependencies: Vec<String>,
        action: Action,
        service: Rc<RateLimitService>,
    ) -> Self {
        Self {
            task_id,
            dependencies,
            is_blocking: true,
            scope: action.scope,
            service,
            conditional_data_sets: action.conditional_data,
            predicates: action.predicates,
            default_hits_addend: 1,
        }
    }

    /// Builds the rate limit descriptors from the context
    fn build_descriptors(&self, ctx: &ReqRespCtx) -> Result<BuildDescriptorsState, AttributeError> {
        // TODO: Candidate for a `prepare` task/method
        // Build descriptor entries by evaluating conditional data
        let mut entries = Vec::new();
        // if predicates don't apply, skip RL. return empty entries
        match self.predicates_apply(ctx) {
            Ok(AttributeState::Available(true)) => {
                // Top level predicates passed, evaluating conditional data to build entries
                for conditional_data in &self.conditional_data_sets {
                    match self.build_entries(conditional_data, ctx) {
                        Ok(cond_entries) => entries.extend(cond_entries),
                        Err(err) => return Err(err),
                    }
                }
            }
            Ok(AttributeState::Available(false)) => {
                // Top level predicates didn't apply, returning empty descriptor entries
                return Ok(BuildDescriptorsState::Ready(vec![]));
            }
            Ok(AttributeState::Pending) => {
                // Can't evaluate yet, need to defer
                return Ok(BuildDescriptorsState::Pending);
            }
            Err(eval_err) => {
                return Err(AttributeError::Retrieval(format!(
                    "Predicate evaluation failed: {}",
                    eval_err
                )));
            }
        }

        // Filter out known attributes from entries
        entries.retain(|entry| !KNOWN_ATTRIBUTES.contains(&entry.key.as_str()));

        // If no entries, return empty vector (no rate limiting needed)
        if entries.is_empty() {
            return Ok(BuildDescriptorsState::Ready(Vec::new()));
        }

        // Build the descriptor
        let descriptor = RateLimitDescriptor {
            entries,
            limit: None,
        };

        // Encode to protobuf bytes
        Ok(BuildDescriptorsState::Ready(vec![descriptor]))
    }

    /// Check if all predicates evaluate to true
    fn predicates_apply(&self, ctx: &ReqRespCtx) -> PredicateResult {
        if self.predicates.is_empty() {
            return Ok(AttributeState::Available(true));
        }

        for predicate in &self.predicates {
            match predicate.test(ctx)? {
                AttributeState::Available(true) => continue,
                AttributeState::Available(false) => return Ok(AttributeState::Available(false)),
                AttributeState::Pending => return Ok(AttributeState::Pending),
            }
        }
        Ok(AttributeState::Available(true))
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

    /// Builds rate limit entries needed for rate limit descriptors
    fn build_entries(
        &self,
        conditional_data: &ConditionalData,
        ctx: &ReqRespCtx,
    ) -> Result<Vec<rate_limit_descriptor::Entry>, AttributeError> {
        if conditional_data.predicates.is_empty()
            || conditional_data.predicates.iter().any(|expr| {
                expr.eval(ctx)
                    .is_ok_and(|v| matches!(v, AttributeState::Available(Value::Bool(true))))
            })
        {
            conditional_data
                .data
                .iter()
                .map(|data_item| {
                    DescriptorEntryBuilder::new(data_item.key.clone(), data_item.value.clone())
                        .evaluate(ctx)
                })
                .collect::<Result<Vec<_>, _>>()
        } else {
            Ok(vec![])
        }
    }
}

impl Task for RateLimitTask {
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        // Build the rate limit descriptors
        let descriptors = match self.build_descriptors(ctx) {
            Ok(BuildDescriptorsState::Ready(msg)) => msg,
            Ok(BuildDescriptorsState::Pending) => {
                // Need to wait for attributes, requeue
                return TaskOutcome::Requeued(self);
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
                is_blocking: self.is_blocking,
                process_response: Box::new(move |response| {
                    // Parse the rate limit response
                    let rate_limit_response = match service.parse_message(response) {
                        Ok(parsed) => parsed,
                        Err(_e) => {
                            // TODO: Handle parsing error and/or FailureMode task?
                            return Vec::new();
                        }
                    };

                    // Process based on response code
                    match rate_limit_response.overall_code {
                        code if code == rate_limit_response::Code::Ok as i32 => {
                            // Rate limit check passed
                            // TODO: Extract headers and push ModifyHeadersTask + DirectResponseTask
                            Vec::new()
                        }
                        code if code == rate_limit_response::Code::OverLimit as i32 => {
                            // Rate limit exceeded - return 429
                            // TODO: Extract headers and push ModifyHeadersTask + DirectResponseTask
                            Vec::new()
                        }
                        _ => {
                            // Unknown or error response
                            // TODO: Handle parsing error and/or FailureMode task?
                            Vec::new()
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::configuration;
    use crate::v2::configuration::ServiceType;
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
            "test".to_string(),
            "test".to_string(),
            std::time::Duration::from_secs(1),
        )
    }

    fn create_test_action(
        predicates: Vec<Predicate>,
        conditional_data: Vec<ConditionalData>,
    ) -> Action {
        Action {
            service: Rc::new(configuration::Service {
                service_type: ServiceType::RateLimit,
                endpoint: "test".to_string(),
                failure_mode: Default::default(),
                timeout: Default::default(),
            }),
            scope: "test".to_string(),
            predicates,
            conditional_data,
        }
    }

    /*
    // TODO: Fix this test
    #[test]
    fn test_descriptor_builder_with_headers() {
        let ctx = create_test_context_with_headers(vec![
            ("host".to_string(), "example.com".to_string()),
        ]);
        let expression = Expression::new("request.headers.host").unwrap();
        let builder = DescriptorEntryBuilder::new("host_key".to_string(), expression);

        let result = builder.evaluate(&ctx).unwrap();

        assert_eq!(result.key, "host_key");
        assert_eq!(result.value, "example.com");
    } */

    #[test]
    fn test_no_predicates_returns_true() {
        let ctx = create_test_context();
        let task = RateLimitTask::new(
            "test".to_string(),
            vec![],
            create_test_action(vec![], vec![]),
            Rc::new(create_test_service()),
        );

        let result = task.predicates_apply(&ctx).unwrap();

        assert_eq!(result, AttributeState::Available(true));
    }

    #[test]
    fn test_predicates_pass() {
        let ctx = create_test_context();
        let predicates = vec![
            Predicate::new("true").unwrap(),
            Predicate::new("1 == 1").unwrap(),
        ];
        let task = RateLimitTask::new(
            "test".to_string(),
            vec![],
            create_test_action(predicates, vec![]),
            Rc::new(create_test_service()),
        );

        let result = task.predicates_apply(&ctx).unwrap();

        assert_eq!(result, AttributeState::Available(true));
    }

    #[test]
    fn test_predicates_one_fails() {
        let ctx = create_test_context();
        let predicates = vec![
            Predicate::new("true").unwrap(),
            Predicate::new("false").unwrap(),
        ];
        let task = RateLimitTask::new(
            "test".to_string(),
            vec![],
            create_test_action(predicates, vec![]),
            Rc::new(create_test_service()),
        );

        let result = task.predicates_apply(&ctx).unwrap();

        assert_eq!(result, AttributeState::Available(false));
    }

    #[test]
    fn test_default_values_when_no_known_attributes() {
        let ctx = create_test_context();
        let task = RateLimitTask::new(
            "test".to_string(),
            vec![],
            create_test_action(vec![], vec![]),
            Rc::new(create_test_service()),
        );

        let (hits_addend, domain) = task.get_known_attributes(&ctx).unwrap();

        assert_eq!(hits_addend, 1);
        assert_eq!(domain, "");
    }

    #[test]
    fn test_default_values_with_known_attributes() {
        let ctx = create_test_context();
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
        let task = RateLimitTask::new(
            "test".to_string(),
            vec![],
            create_test_action(vec![], vec![conditional_data]),
            Rc::new(create_test_service()),
        );

        let (hits_addend, domain) = task.get_known_attributes(&ctx).unwrap();

        assert_eq!(hits_addend, 5);
        assert_eq!(domain, "example.org");
    }

    #[test]
    fn test_build_entries() {
        let ctx = create_test_context();
        let conditional_data = ConditionalData {
            predicates: vec![],
            data: vec![
                DataItem {
                    key: "key1".to_string(),
                    value: Expression::new("\"value1\"").unwrap(),
                },
                DataItem {
                    key: "key2".to_string(),
                    value: Expression::new("\"value2\"").unwrap(),
                },
            ],
        };
        let task = RateLimitTask::new(
            "test".to_string(),
            vec![],
            create_test_action(
                vec![],
                vec![ConditionalData {
                    predicates: vec![],
                    data: vec![
                        DataItem {
                            key: "key1".to_string(),
                            value: Expression::new("\"value1\"").unwrap(),
                        },
                        DataItem {
                            key: "key2".to_string(),
                            value: Expression::new("\"value2\"").unwrap(),
                        },
                    ],
                }],
            ),
            Rc::new(create_test_service()),
        );

        let entries = task.build_entries(&conditional_data, &ctx).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].key, "key1");
        assert_eq!(entries[0].value, "value1");
        assert_eq!(entries[1].key, "key2");
        assert_eq!(entries[1].value, "value2");
    }

    #[test]
    fn test_build_descriptors_filters_known_attributes() {
        let ctx = create_test_context();
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
        let task = RateLimitTask::new(
            "test".to_string(),
            vec![],
            create_test_action(vec![], vec![conditional_data]),
            Rc::new(create_test_service()),
        );

        let result = task.build_descriptors(&ctx).unwrap();

        match result {
            BuildDescriptorsState::Ready(descriptors) => {
                assert_eq!(descriptors.len(), 1);
                // Known attributes should be filtered out
                assert_eq!(descriptors[0].entries.len(), 1);
                assert_eq!(descriptors[0].entries[0].key, "actual_key");
            }
            BuildDescriptorsState::Pending => unreachable!("Expected Ready, got Pending"),
        }
    }

    #[test]
    fn test_build_descriptors_with_failing_predicate() {
        let ctx = create_test_context();
        let conditional_data = ConditionalData {
            predicates: vec![],
            data: vec![DataItem {
                key: "test_key".to_string(),
                value: Expression::new("\"test_value\"").unwrap(),
            }],
        };
        let task = RateLimitTask::new(
            "test".to_string(),
            vec![],
            create_test_action(
                vec![Predicate::new("false").unwrap()],
                vec![conditional_data],
            ),
            Rc::new(create_test_service()),
        );

        let result = task.build_descriptors(&ctx).unwrap();

        match result {
            BuildDescriptorsState::Ready(descriptors) => {
                // Predicate failed, so no descriptors should be built
                assert_eq!(descriptors.len(), 0);
            }
            BuildDescriptorsState::Pending => unreachable!("Expected Ready, got Pending"),
        }
    }

    #[test]
    fn test_build_descriptors_with_passing_predicate() {
        let ctx = create_test_context();
        let conditional_data = ConditionalData {
            predicates: vec![],
            data: vec![DataItem {
                key: "test_key".to_string(),
                value: Expression::new("\"test_value\"").unwrap(),
            }],
        };
        let task = RateLimitTask::new(
            "test".to_string(),
            vec![],
            create_test_action(
                vec![Predicate::new("true").unwrap()],
                vec![conditional_data],
            ),
            Rc::new(create_test_service()),
        );

        let result = task.build_descriptors(&ctx).unwrap();

        match result {
            BuildDescriptorsState::Ready(descriptors) => {
                assert_eq!(descriptors.len(), 1);
                assert_eq!(descriptors[0].entries.len(), 1);
            }
            BuildDescriptorsState::Pending => unreachable!("Expected Ready, got Pending"),
        }
    }

    #[test]
    fn test_build_descriptors_with_failing_conditional_data_predicate() {
        let ctx = create_test_context();
        let conditional_data = ConditionalData {
            predicates: vec![Expression::new("false").unwrap()],
            data: vec![DataItem {
                key: "test_key".to_string(),
                value: Expression::new("\"test_value\"").unwrap(),
            }],
        };
        let task = RateLimitTask::new(
            "test".to_string(),
            vec![],
            create_test_action(vec![], vec![conditional_data]),
            Rc::new(create_test_service()),
        );

        let result = task.build_descriptors(&ctx).unwrap();

        match result {
            BuildDescriptorsState::Ready(descriptors) => {
                assert_eq!(descriptors.len(), 0);
            }
            BuildDescriptorsState::Pending => unreachable!("Expected Ready, got Pending"),
        }
    }

    #[test]
    fn test_build_descriptors_with_passing_conditional_data_predicate() {
        let ctx = create_test_context();
        let conditional_data = ConditionalData {
            predicates: vec![Expression::new("true").unwrap()],
            data: vec![DataItem {
                key: "test_key".to_string(),
                value: Expression::new("\"test_value\"").unwrap(),
            }],
        };
        let task = RateLimitTask::new(
            "test".to_string(),
            vec![],
            create_test_action(vec![], vec![conditional_data]),
            Rc::new(create_test_service()),
        );

        let result = task.build_descriptors(&ctx).unwrap();

        match result {
            BuildDescriptorsState::Ready(descriptors) => {
                assert_eq!(descriptors.len(), 1);
                assert_eq!(descriptors[0].entries.len(), 1);
            }
            BuildDescriptorsState::Pending => unreachable!("Expected Ready, got Pending"),
        }
    }

    #[test]
    fn test_task_outcome_done() {
        let mut ctx = create_test_context();

        let task = Box::new(RateLimitTask::new(
            "test".to_string(),
            vec![],
            create_test_action(vec![Predicate::new("false").unwrap()], vec![]),
            Rc::new(create_test_service()),
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

        let task = Box::new(RateLimitTask::new(
            "test".to_string(),
            vec![],
            create_test_action(
                vec![Predicate::new("true").unwrap()],
                vec![conditional_data],
            ),
            Rc::new(create_test_service()),
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
