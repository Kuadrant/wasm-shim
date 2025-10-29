#[allow(dead_code)]
use crate::v2::kuadrant::pipeline::tasks::{PendingTask, ResponseProcessor, Task, TaskOutcome};
use crate::v2::kuadrant::ReqRespCtx;
use crate::v2::services::{Service, ServiceError};
use crate::v2::data::cel::{Expression, Predicate, EvaluationError};
use crate::v2::data::attribute::{AttributeState, AttributeError};
use crate::envoy::{
    rate_limit_descriptor, RateLimitDescriptor, RateLimitRequest, RateLimitResponse,
    rate_limit_response
};
use cel_interpreter::Value;
use prost::Message;
use std::rc::Rc;

/// Represents a set of descriptor entries with predicates
struct ConditionalData {
    data: Vec<DescriptorEntryBuilder>,
    predicates: Vec<Predicate>,
}

impl ConditionalData {
    /// Check if all predicates evaluate to true
    fn predicates_apply(&self, ctx: &ReqRespCtx) -> Result<AttributeState<bool>, EvaluationError> {
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

    /// Build descriptor entries by evaluating expressions
    fn build_entries(&self, ctx: &ReqRespCtx) -> Result<Vec<rate_limit_descriptor::Entry>, AttributeError> {
        let mut entries = Vec::new();

        for builder in &self.data {
            entries.push(builder.evaluate(ctx)?);
        }

        Ok(entries)
    }
}

/// Builds individual descriptor entries from CEL expressions
struct DescriptorEntryBuilder {
    key: String,
    expression: Expression,
}

impl DescriptorEntryBuilder {
    /// Evaluate the expression to create a descriptor entry
    fn evaluate(&self, ctx: &ReqRespCtx) -> Result<rate_limit_descriptor::Entry, AttributeError> {
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
                        return Err(AttributeError::Parse("Only scalar values can be sent as data".to_string()));
                    }
                };

                Ok(rate_limit_descriptor::Entry {
                    key: self.key.clone(),
                    value: value_str,
                })
            }
            Ok(AttributeState::Pending) => {
                Err(AttributeError::NotAvailable(
                    format!("Expression '{}' evaluation is pending", self.key)
                ))
            }
            Err(cel_err) => {
                Err(AttributeError::Retrieval(
                    format!("CEL evaluation error for '{}': {}", self.key, cel_err)
                ))
            }
        }
    }
}

/// Result type indicating the message build outcome
enum BuildMessageResult {
    Ready(Vec<u8>),
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
    service: Rc<dyn Service<Response = RateLimitResponse>>,

    // Conditional data for building descriptors
    conditional_data_sets: Vec<ConditionalData>,

    // Default hits addend
    default_hits_addend: u32,
}

#[allow(dead_code)]
impl RateLimitTask {
    fn new(
        task_id: String,
        dependencies: Vec<String>,
        is_blocking: bool,
        scope: String,
        service: Rc<dyn Service<Response = RateLimitResponse>>,
        conditional_data_sets: Vec<ConditionalData>,
    ) -> Self {
        Self {
            task_id,
            dependencies,
            is_blocking,
            scope,
            service,
            conditional_data_sets,
            default_hits_addend: 1,
        }
    }

    /// Builds the rate limit request message from the context
    fn build_message(&self, ctx: &ReqRespCtx) -> BuildMessageResult {
        // Build descriptor entries by evaluating conditional data
        let mut entries = Vec::new();

        for conditional_data in &self.conditional_data_sets {
            match conditional_data.predicates_apply(ctx) {
                Ok(AttributeState::Available(true)) => {
                    // Predicates passed, evaluate entries
                    match conditional_data.build_entries(ctx) {
                        Ok(cond_entries) => entries.extend(cond_entries),
                        Err(err) => return BuildMessageResult::Error(err),
                    }
                }
                Ok(AttributeState::Available(false)) => {
                    // Predicates didn't pass, skip this conditional data
                    continue;
                }
                Ok(AttributeState::Pending) => {
                    // Can't evaluate yet, need to defer
                    return BuildMessageResult::Pending;
                }
                Err(eval_err) => {
                    return BuildMessageResult::Error(AttributeError::Retrieval(
                        format!("Predicate evaluation failed: {}", eval_err)
                    ));
                }
            }
        }

        // Extract known attributes (hits_addend, domain) before filtering
        let (hits_addend, domain_override) = match self.get_known_attributes(ctx) {
            Ok(attrs) => attrs,
            Err(err) => return BuildMessageResult::Error(err),
        };

        // Filter out known attributes from entries
        entries.retain(|entry| !KNOWN_ATTRIBUTES.contains(&entry.key.as_str()));

        // If no entries, return empty vector (no rate limiting needed)
        if entries.is_empty() {
            return BuildMessageResult::Ready(Vec::new());
        }

        // Determine domain (use override or default scope)
        let domain = if domain_override.is_empty() {
            self.scope.clone()
        } else {
            domain_override
        };

        // Build the descriptor
        let descriptor = RateLimitDescriptor {
            entries,
            limit: None,
        };

        // Build the rate limit request
        let request = RateLimitRequest {
            domain,
            descriptors: vec![descriptor],
            hits_addend,
        };

        // Encode to protobuf bytes
        BuildMessageResult::Ready(request.encode_to_vec())
    }

    /// Extract known attributes like ratelimit.domain and ratelimit.hits_addend
    fn get_known_attributes(&self, ctx: &ReqRespCtx) -> Result<(u32, String), AttributeError> {
        let mut hits_addend = self.default_hits_addend;
        let mut domain = String::new();

        for conditional_data in &self.conditional_data_sets {
            for entry_builder in &conditional_data.data {
                if KNOWN_ATTRIBUTES.contains(&entry_builder.key.as_str()) {
                    match entry_builder.expression.eval(ctx) {
                        Ok(AttributeState::Available(val)) => {
                            match entry_builder.key.as_str() {
                                "ratelimit.domain" => {
                                    if let Value::String(s) = val {
                                        if s.is_empty() {
                                            return Err(AttributeError::Parse(
                                                "ratelimit.domain cannot be empty".to_string()
                                            ));
                                        }
                                        domain = s.to_string();
                                    } else {
                                        return Err(AttributeError::Parse("ratelimit.domain must be string".to_string()));
                                    }
                                }
                                "ratelimit.hits_addend" => {
                                    match val {
                                        Value::Int(i) if i >= 0 && i <= u32::MAX as i64 => {
                                            hits_addend = i as u32;
                                        }
                                        Value::UInt(u) if u <= u32::MAX as u64 => {
                                            hits_addend = u as u32;
                                        }
                                        _ => {
                                            return Err(AttributeError::Parse(
                                                "ratelimit.hits_addend must be 0 <= X <= u32::MAX".to_string()
                                            ));
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                        Ok(AttributeState::Pending) => {
                            return Err(AttributeError::NotAvailable(
                                format!("Attribute {} is pending", entry_builder.key)
                            ));
                        }
                        Err(cel_err) => {
                            return Err(AttributeError::Retrieval(
                                format!("CEL evaluation error: {}", cel_err)
                            ));
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
        // Build the rate limit message
        let message = match self.build_message(ctx) {
            BuildMessageResult::Ready(msg) => msg,
            BuildMessageResult::Pending => {
                // Need to wait for attributes, requeue
                return TaskOutcome::Requeued(self);
            }
            BuildMessageResult::Error(_e) => {
                // TODO: Handle error appropriately based on failure mode
                return TaskOutcome::Failed;
            }
        };

        // If empty message, skip rate limiting... Or should it be TaskOutcome::Failed?
        if message.is_empty() {
            return TaskOutcome::Done;
        }

        // Dispatch the message to the rate limit service
        let token_id = match self.service.dispatch(ctx, message) {
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
