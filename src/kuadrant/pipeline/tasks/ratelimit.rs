use crate::data::attribute::{AttributeError, AttributeState};
use crate::data::cel::errors::EvaluationError;
use crate::data::cel::{Expression, Predicate, PredicateVec};
use crate::data::Headers;
use crate::envoy::rate_limit_descriptor::Entry;
use crate::envoy::{rate_limit_response, HeaderValue, RateLimitDescriptor, RateLimitResponse};
use crate::kuadrant::pipeline::blueprint::ConditionalData;
use crate::kuadrant::pipeline::tasks::{
    HeaderOperation, HeadersType, ModifyHeadersTask, SendReplyTask,
};
use crate::kuadrant::pipeline::tasks::{PendingTask, Task, TaskOutcome};
use crate::kuadrant::ReqRespCtx;
use crate::services::{RateLimitService, Service};
use cel_interpreter::Value;
use log::{debug, error};
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
}

/// Creates a new RL task
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
        }
    }

    /// Creates a new RL task prior caching its needed attributes
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_attributes(
        ctx: &ReqRespCtx,
        task_id: String,
        dependencies: Vec<String>,
        service: Rc<RateLimitService>,
        scope: String,
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
                AttributeState::Pending => {
                    for data_item in &conditional_data.data {
                        let _ = DescriptorEntryBuilder::new(
                            data_item.key.clone(),
                            data_item.value.clone(),
                        )
                        .evaluate(ctx);
                    }
                    return Ok(AttributeState::Pending);
                }
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
        const DEFAULT_HITS_ADDEND: u32 = 1;
        let mut hits_addend = DEFAULT_HITS_ADDEND;
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
    #[tracing::instrument(name = "ratelimit", skip(self, ctx), fields(task_id = %self.task_id))]
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
            Err(e) => {
                error!("Failed to build descriptors: {e:?}");
                return TaskOutcome::Failed;
            }
        };

        if descriptors.is_empty() {
            debug!("No descriptors to rate limit");
            return TaskOutcome::Done;
        }

        // Extract known attributes (hits_addend, domain) before filtering
        let (hits_addend, domain_override) = match self.get_known_attributes(ctx) {
            Ok(attrs) => attrs,
            Err(e) => {
                error!("Failed to extract known attributes: {e:?}");
                return TaskOutcome::Failed;
            }
        };

        // Determine domain (use override or default scope)
        let domain: String = if domain_override.is_empty() {
            self.scope.clone()
        } else {
            domain_override
        };

        // Dispatch the rate limit service message
        let token_id = {
            let _span = tracing::debug_span!("ratelimit_request").entered();
            match self
                .service
                .dispatch_ratelimit(ctx, &domain, descriptors, hits_addend)
            {
                Ok(id) => id,
                Err(e) => {
                    error!("Failed to dispatch rate limit: {}", e);
                    return TaskOutcome::Failed;
                }
            }
        };

        // Prepare response processing
        let service = self.service.clone();

        // Capture the current span context to link the response processing
        let parent_span = tracing::Span::current();

        // Return deferred outcome with response processor
        TaskOutcome::Deferred {
            token_id,
            pending: Box::new(PendingTask {
                task_id: self.task_id,
                process_response: Box::new(move |ctx| {
                    let span = tracing::debug_span!(parent: parent_span.id(), "ratelimit_response");
                    let _guard = span.enter();
                    match ctx.get_grpc_response_data() {
                        Ok((status_code, response_size)) => {
                            if status_code != proxy_wasm::types::Status::Ok as u32 {
                                TaskOutcome::Failed
                            } else {
                                match service.get_response(ctx, response_size) {
                                    Ok(response) => process_rl_response(response),
                                    Err(e) => {
                                        error!("Failed to get response: {e:?}");
                                        TaskOutcome::Failed
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            error!("Failed to get response: {e:?}");
                            TaskOutcome::Failed
                        }
                    }
                }),
            }),
        }
    }

    fn id(&self) -> Option<String> {
        Some(self.task_id.clone())
    }

    fn dependencies(&self) -> &[String] {
        self.dependencies.as_slice()
    }

    fn pauses_filter(&self) -> bool {
        self.pauses_filter
    }
}

fn process_rl_response(response: RateLimitResponse) -> TaskOutcome {
    // Process based on response code
    match response.overall_code {
        code if code == rate_limit_response::Code::Ok as i32 => {
            // Rate limit check passed
            if !response.response_headers_to_add.is_empty() {
                let headers = from_envoy_header_value(&response.response_headers_to_add);
                return TaskOutcome::Requeued(vec![Box::new(ModifyHeadersTask::new(
                    HeaderOperation::Append(headers),
                    HeadersType::HttpResponseHeaders,
                ))]);
            }
            TaskOutcome::Done
        }
        code if code == rate_limit_response::Code::OverLimit as i32 => {
            // Rate limit exceeded - return 429
            let headers = from_envoy_header_value(&response.response_headers_to_add);
            let status_code = crate::envoy::StatusCode::TooManyRequests as u32;
            let body = Some("Too Many Requests\n".to_string());

            TaskOutcome::Terminate(Box::new(SendReplyTask::new(
                status_code,
                headers.into_inner(),
                body,
            )))
        }
        i32::MIN..=i32::MAX => {
            // Unknown code or error response
            TaskOutcome::Failed
        }
    }
}

pub fn from_envoy_header_value(headers: &[HeaderValue]) -> Headers {
    let vec: Vec<(String, String)> = headers
        .iter()
        .map(|hv| (hv.key.to_owned(), hv.value.to_owned()))
        .collect();
    vec.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configuration::FailureMode;
    use crate::data::cel::Expression;
    use crate::data::cel::Predicate;
    use crate::kuadrant::pipeline::blueprint::DataItem;
    use crate::kuadrant::resolver::MockWasmHost;
    use crate::kuadrant::ReqRespCtx;
    use std::sync::Arc;

    fn create_test_context() -> ReqRespCtx {
        let mock_host = MockWasmHost::new();
        ReqRespCtx::new(Arc::new(mock_host))
    }

    fn create_test_service() -> RateLimitService {
        RateLimitService::new(
            "test".to_string(),
            std::time::Duration::from_secs(1),
            "test",
            "POST",
            FailureMode::Deny,
        )
    }

    fn create_test_task_with(
        ctx: &ReqRespCtx,
        top_predicates: Vec<Predicate>,
        conditional_data: Vec<ConditionalData>,
    ) -> RateLimitTask {
        RateLimitTask::new_with_attributes(
            ctx,
            "test".to_string(),
            vec![],
            Rc::new(create_test_service()),
            "test".to_string(),
            top_predicates,
            conditional_data,
            false,
        )
    }

    #[test]
    fn test_default_values_when_no_known_attributes() {
        let ctx = create_test_context();
        let task = RateLimitTask::new_with_attributes(
            &ctx,
            "test".to_string(),
            vec![],
            Rc::new(create_test_service()),
            "test".to_string(),
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
        let task = RateLimitTask::new_with_attributes(
            &ctx,
            "test".to_string(),
            vec![],
            Rc::new(create_test_service()),
            "test".to_string(),
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
        let task = RateLimitTask::new_with_attributes(
            &ctx,
            "test".to_string(),
            vec![],
            Rc::new(create_test_service()),
            "test".to_string(),
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
        let ctx = create_test_context();
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
            &ctx,
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
        let ctx = create_test_context();
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
            &ctx,
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
        let ctx = create_test_context();
        let conditional_data = ConditionalData {
            predicates: vec![Predicate::new("false").unwrap()],
            data: vec![DataItem {
                key: "test_key".to_string(),
                value: Expression::new("\"test_value\"").unwrap(),
            }],
        };
        let task = create_test_task_with(&ctx, vec![], vec![conditional_data]);

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
        let ctx = create_test_context();
        let conditional_data = ConditionalData {
            predicates: vec![Predicate::new("true").unwrap()],
            data: vec![DataItem {
                key: "test_key".to_string(),
                value: Expression::new("\"test_value\"").unwrap(),
            }],
        };
        let task = create_test_task_with(&ctx, vec![], vec![conditional_data]);

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
            &ctx,
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
            &ctx,
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
    // TODO: More specific testing for task outcomes
}
