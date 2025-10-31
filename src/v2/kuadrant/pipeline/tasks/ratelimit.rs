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

/// Result type indicating the message build outcome
enum BuildDescriptorsResult {
    Ready(Vec<RateLimitDescriptor>),
    Pending,
    Error(AttributeError),
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
    fn build_descriptors(&self, ctx: &ReqRespCtx) -> BuildDescriptorsResult {
        // Build descriptor entries by evaluating conditional data
        let mut entries = Vec::new();

        // TODO: Candidate for a `prepare` task/method
        for conditional_data in &self.conditional_data_sets {
            match self.predicates_apply(ctx) {
                Ok(AttributeState::Available(true)) => {
                    // Predicates passed, evaluate entries
                    match self.build_entries(conditional_data, ctx) {
                        Ok(cond_entries) => entries.extend(cond_entries),
                        Err(err) => return BuildDescriptorsResult::Error(err),
                    }
                }
                Ok(AttributeState::Available(false)) => {
                    // Predicates didn't pass, skip this conditional data
                    continue;
                }
                Ok(AttributeState::Pending) => {
                    // Can't evaluate yet, need to defer
                    return BuildDescriptorsResult::Pending;
                }
                Err(eval_err) => {
                    return BuildDescriptorsResult::Error(AttributeError::Retrieval(format!(
                        "Predicate evaluation failed: {}",
                        eval_err
                    )));
                }
            }
        }

        // Filter out known attributes from entries
        entries.retain(|entry| !KNOWN_ATTRIBUTES.contains(&entry.key.as_str()));

        // If no entries, return empty vector (no rate limiting needed)
        if entries.is_empty() {
            return BuildDescriptorsResult::Ready(Vec::new());
        }

        // Build the descriptor
        let descriptor = RateLimitDescriptor {
            entries,
            limit: None,
        };

        // Encode to protobuf bytes
        BuildDescriptorsResult::Ready(vec![descriptor])
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
        conditional_data
            .data
            .iter()
            .map(|data_item| {
                DescriptorEntryBuilder::new(data_item.key.clone(), data_item.value.clone())
                    .evaluate(ctx)
            })
            .collect::<Result<Vec<_>, _>>()
    }
}

impl Task for RateLimitTask {
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        // Build the rate limit message
        let descriptors = match self.build_descriptors(ctx) {
            BuildDescriptorsResult::Ready(msg) => msg,
            BuildDescriptorsResult::Pending => {
                // Need to wait for attributes, requeue
                return TaskOutcome::Requeued(self);
            }
            BuildDescriptorsResult::Error(_e) => {
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
                            // TODO: Handle parsing error based on failure mode
                            return Vec::new();
                        }
                    };

                    // Process based on response code
                    match rate_limit_response.overall_code {
                        code if code == rate_limit_response::Code::Ok as i32 => {
                            // Rate limit check passed
                            // TODO: Extract headers and create AddResponseHeaders task
                            Vec::new()
                        }
                        code if code == rate_limit_response::Code::OverLimit as i32 => {
                            // Rate limit exceeded - return 429 task
                            // TODO: Extract headers and refine TooManyRequestsTask task
                            Vec::new()
                        }
                        _ => {
                            // Unknown or error response
                            // TODO: Handle based on failure mode
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
#[allow(dead_code)]
struct TooManyRequestsTask {}

impl Task for TooManyRequestsTask {
    fn apply(self: Box<Self>, _: &mut ReqRespCtx) -> TaskOutcome {
        // ctx.send_message 429
        TaskOutcome::Done
    }
}
