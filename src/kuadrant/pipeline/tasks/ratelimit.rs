use crate::data::attribute::{AttributeError, AttributeState};
use crate::data::cel::errors::EvaluationError;
use crate::data::cel::{Expression, Predicate, PredicateVec};
use crate::envoy::rate_limit_descriptor::Entry;
use crate::envoy::{rate_limit_response, RateLimitDescriptor};
use crate::kuadrant::pipeline::blueprint::ConditionalData;
use crate::kuadrant::pipeline::tasks::{
    HeaderOperation, HeadersType, ModifyHeadersTask, SendReplyTask,
};
use crate::kuadrant::pipeline::tasks::{PendingTask, Task, TaskOutcome};
use crate::kuadrant::ReqRespCtx;
use crate::record_error;
use crate::services::{DynamicService, Service};
use cel::Value;
use prost_reflect::DynamicMessage;
use prost_reflect::Value as ProtoValue;
use std::rc::Rc;
use tracing::{debug, error};

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

    // Rate limit configuration
    scope: String,
    service: Rc<DynamicService>,

    // Conditional data for building descriptors
    conditional_data_sets: Vec<ConditionalData>,
    predicates: Vec<Predicate>,
}

/// Creates a new RL task
impl RateLimitTask {
    /// Creates a new RL task prior caching its needed attributes
    pub fn new_with_attributes(
        ctx: &ReqRespCtx,
        task_id: String,
        dependencies: Vec<String>,
        service: Rc<DynamicService>,
        scope: String,
        predicates: Vec<Predicate>,
        conditional_data_sets: Vec<ConditionalData>,
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

        Self {
            task_id,
            dependencies,
            scope,
            service,
            conditional_data_sets,
            predicates,
        }
    }

    fn add_request_data_entries(
        &self,
        ctx: &mut ReqRespCtx,
        mut descriptors: Vec<RateLimitDescriptor>,
    ) -> Vec<RateLimitDescriptor> {
        let request_data = ctx.eval_request_data();
        if !request_data.is_empty() {
            let entries: Vec<_> = request_data
                .iter()
                .filter_map(|entry| match &entry.result {
                    Ok(AttributeState::Available(Value::String(value))) => {
                        let key = if entry.domain.is_empty() || entry.domain == "metrics.labels" {
                            entry.field.clone()
                        } else {
                            format!("{}.{}", entry.domain, entry.field)
                        };
                        Some(Entry {
                            key,
                            value: value.to_string(),
                        })
                    }
                    _ => None,
                })
                .collect();

            if !entries.is_empty() {
                descriptors.push(RateLimitDescriptor {
                    entries,
                    limit: None,
                });
            }
        }
        descriptors
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
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        match self.predicates.apply(ctx) {
            Ok(AttributeState::Pending) => {
                return if ctx.is_end_of_stream() {
                    TaskOutcome::Failed
                } else {
                    TaskOutcome::Requeued(vec![self])
                };
            }
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
                return if ctx.is_end_of_stream() {
                    TaskOutcome::Failed
                } else {
                    TaskOutcome::Requeued(vec![self])
                };
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

        let descriptors = self.add_request_data_entries(ctx, descriptors);
        let cel_expr = generate_ratelimit_request_cel(&domain, hits_addend, &descriptors);

        // Dispatch the rate limit service message
        let token_id = {
            let _span =
                tracing::debug_span!("ratelimit_request", task_id = self.task_id, scope = domain)
                    .entered();
            #[allow(deprecated)]
            match self.service.dispatch_dynamic(ctx, &cel_expr) {
                Ok(id) => id,
                Err(e) => {
                    error!("Failed to dispatch rate limit: {}", e);
                    return TaskOutcome::Failed;
                }
            }
        };

        // Prepare response processing
        let service = self.service.clone();
        let task_id = self.task_id.clone();

        // Return deferred outcome with response processor
        TaskOutcome::Deferred {
            token_id,
            pending: Box::new(PendingTask::new(
                self.task_id,
                Box::new(move |ctx| {
                    let span = tracing::debug_span!(
                        "ratelimit_response",
                        task_id = task_id,
                        token_id = token_id,
                        grpc_status_code = tracing::field::Empty,
                        otel.status_code = tracing::field::Empty,
                        otel.status_message = tracing::field::Empty
                    )
                    .entered();
                    match ctx.get_grpc_response_data() {
                        Ok((status_code, response_size)) => {
                            span.record("grpc_status_code", status_code);
                            if status_code != proxy_wasm::types::Status::Ok as u32 {
                                record_error!("gRPC status code is not OK");
                                TaskOutcome::Failed
                            } else {
                                match service.get_response(ctx, response_size) {
                                    Ok(response) => process_rl_response(response),
                                    Err(e) => {
                                        record_error!("Failed to get response: {e:?}");
                                        TaskOutcome::Failed
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            record_error!("Failed to get response: {e:?}");
                            TaskOutcome::Failed
                        }
                    }
                }),
            )),
        }
    }

    fn id(&self) -> Option<String> {
        Some(self.task_id.clone())
    }

    fn dependencies(&self) -> &[String] {
        self.dependencies.as_slice()
    }
}

fn process_rl_response(response: DynamicMessage) -> TaskOutcome {
    // Extract overall_code
    let overall_code = match response.get_field_by_name("overall_code") {
        Some(field) => match field.as_ref() {
            ProtoValue::I32(code) => *code,
            ProtoValue::EnumNumber(code) => *code,
            _ => {
                error!("overall_code is not an integer");
                return TaskOutcome::Failed;
            }
        },
        None => {
            error!("overall_code field not found");
            return TaskOutcome::Failed;
        }
    };

    // Extract response headers
    let response_headers = match response.get_field_by_name("response_headers_to_add") {
        Some(field) => match field.as_ref() {
            ProtoValue::List(headers_list) => {
                let mut headers = Vec::new();
                for header_item in headers_list {
                    if let ProtoValue::Message(header_msg) = header_item {
                        let key = match header_msg.get_field_by_name("key") {
                            Some(k) => match k.as_ref() {
                                ProtoValue::String(s) => s.to_string(),
                                _ => continue,
                            },
                            None => continue,
                        };
                        let value = match header_msg.get_field_by_name("value") {
                            Some(v) => match v.as_ref() {
                                ProtoValue::String(s) => s.to_string(),
                                _ => continue,
                            },
                            None => continue,
                        };
                        headers.push((key, value));
                    }
                }
                headers
            }
            _ => Vec::new(),
        },
        None => Vec::new(),
    };

    // Process based on response code
    if overall_code == rate_limit_response::Code::Ok as i32 {
        if !response_headers.is_empty() {
            return TaskOutcome::Requeued(vec![Box::new(ModifyHeadersTask::new(
                HeaderOperation::Set(response_headers.into()),
                HeadersType::HttpResponseHeaders,
            ))]);
        }
        TaskOutcome::Done
    } else if overall_code == rate_limit_response::Code::OverLimit as i32 {
        let status_code = crate::envoy::StatusCode::TooManyRequests as u32;
        let body = Some("Too Many Requests\n".to_string());

        TaskOutcome::Terminate(Box::new(SendReplyTask::new(
            status_code,
            response_headers,
            body,
        )))
    } else {
        // Unknown code or error response
        TaskOutcome::Failed
    }
}

fn generate_ratelimit_request_cel(
    domain: &str,
    hits_addend: u32,
    descriptors: &[RateLimitDescriptor],
) -> String {
    fn escape_string(s: &str) -> String {
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t")
    }

    let descriptors_cel: Vec<String> = descriptors
        .iter()
        .map(|descriptor| {
            let entries_cel: Vec<String> = descriptor
                .entries
                .iter()
                .map(|entry| {
                    format!(
                        "envoy.extensions.common.ratelimit.v3.RateLimitDescriptor.Entry {{ key: \"{}\", value: \"{}\" }}",
                        escape_string(&entry.key),
                        escape_string(&entry.value)
                    )
                })
                .collect();

            format!(
                "envoy.extensions.common.ratelimit.v3.RateLimitDescriptor {{ entries: [{}] }}",
                entries_cel.join(", ")
            )
        })
        .collect();

    format!(
        r#"envoy.service.ratelimit.v3.RateLimitRequest {{
    domain: "{}",
    hits_addend: {}u,
    descriptors: [{}]
}}"#,
        escape_string(domain),
        hits_addend,
        descriptors_cel.join(", ")
    )
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

    fn create_test_service() -> DynamicService {
        use crate::filter::DescriptorManager;

        DynamicService::new(
            "test".to_string(),
            "envoy.service.ratelimit.v3.RateLimitService".to_string(),
            "ShouldRateLimit".to_string(),
            std::time::Duration::from_secs(1),
            FailureMode::Deny,
            Rc::new(DescriptorManager::default()),
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
