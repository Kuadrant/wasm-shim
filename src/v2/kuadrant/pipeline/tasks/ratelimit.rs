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

#[allow(dead_code)]
struct RateLimitTask {
    scope: String,
    predicate: Predicate,
    service: Rc<dyn Service<Response = bool>>,
    allow_task: Option<Box<dyn Task>>,
    deny_task: Box<dyn Task>,
}

impl Task for RateLimitTask {
    fn apply(self: Box<Self>, _: &mut ReqRespCtx) -> TaskOutcome {
        // match self.predicate.eval(ctx) taht returns Result<AttributeState<Value>, CelError>
        // if AttributeState(ok) --> self.service.dispatch, TaskOutcome::Deferred
        // else TaskOutcome::Done
        // if err (?) TaskOutcome::Failed(self) ?

        TaskOutcome::Done
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
