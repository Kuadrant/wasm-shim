use cel::common::types::{CelString, CelUInt};
use cel::Value;
use tracing::error;

use crate::data::attribute::AttributeState;
use crate::data::cel::Predicate;
use crate::data::Expression;
use crate::kuadrant::pipeline::tasks::{NoopTerminalTask, Task, TaskOutcome};
use crate::kuadrant::ReqRespCtx;
use crate::services::{cel_value_to_header_pairs, deny_response_struct_def};

pub struct SendReplyTask {
    task_id: String,
    predicate: Predicate,
    deny_with: Expression,
    terminal: bool,
}

impl SendReplyTask {
    pub fn new(
        task_id: String,
        predicate: Predicate,
        deny_with: Expression,
        terminal: bool,
    ) -> Self {
        Self {
            task_id,
            predicate,
            deny_with,
            terminal,
        }
    }

    pub fn default() -> Self {
        #[allow(clippy::expect_used)]
        let deny_with = Expression::new(
            r#"DenyResponse { status: 500u, headers: [], body: 'Internal Server Error.\n' }"#,
        )
        .expect("Needs to be valid CEL!");
        #[allow(clippy::expect_used)]
        let predicate = Predicate::new("true").expect("Needs to be valid!");
        Self {
            task_id: "default".to_string(),
            predicate,
            deny_with,
            terminal: false,
        }
    }
}

impl Task for SendReplyTask {
    fn id(&self) -> &str {
        &self.task_id
    }

    fn cel_types(&self) -> Vec<cel::StructDef> {
        vec![deny_response_struct_def()]
    }

    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        let mut cel_ctx = ctx.cel.new_ctx(&*self);
        match self.predicate.test(ctx, &mut cel_ctx) {
            Ok(AttributeState::Available(true)) => {}
            Ok(AttributeState::Available(false)) => return TaskOutcome::Done,
            Ok(AttributeState::Pending) => {
                return TaskOutcome::Requeued(vec![self]);
            }
            Err(e) => {
                error!("Failed to evaluate predicate: {e:?}");
                return TaskOutcome::Failed;
            }
        }

        let span = tracing::info_span!("send_reply", status_code = tracing::field::Empty);
        let _guard = span.enter();

        let (status_code, headers, body) = {
            match self.deny_with.eval(ctx, &mut cel_ctx) {
                Ok(AttributeState::Pending) => {
                    error!("Unexpected pending state in deny expression");
                    return TaskOutcome::Failed;
                }
                Ok(AttributeState::Available(val @ Value::Struct(_))) => {
                    let Value::Struct(deny_response) = val else {
                        error!("Invalid DenyResponse: {val:?}");
                        return TaskOutcome::Failed;
                    };

                    let status = deny_response
                        .field_value("status")
                        .and_then(|v| v.downcast_ref::<CelUInt>())
                        .map(|v| *v.inner() as u32);

                    let body = deny_response
                        .field_value("body")
                        .and_then(|v| v.downcast_ref::<CelString>())
                        .map(|v| v.inner().to_string())
                        .filter(|s| !s.is_empty());

                    let headers = deny_response
                        .field_value("headers")
                        .and_then(|v| Value::try_from(v).ok())
                        .map(|v| cel_value_to_header_pairs(&v))
                        .unwrap_or_default();
                    (status.unwrap_or(500u32), headers, body)
                }
                Ok(AttributeState::Available(other)) => {
                    error!("denyWith must return DenyResponse, got: {other:?}");
                    return TaskOutcome::Failed;
                }
                Err(e) => {
                    error!("Failed to evaluate denyWith expression: {e}");
                    return TaskOutcome::Failed;
                }
            }
        };

        span.record("status_code", status_code);

        let mut headers_ref: Vec<(&str, &str)> = headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        if let Some((tracker, value)) = ctx.tracker() {
            headers_ref.push((tracker, value));
        }

        ctx.metrics().increment_denied();

        let body_bytes = body.as_ref().map(|s| s.as_bytes());
        match ctx.send_http_reply(status_code, headers_ref, body_bytes) {
            Ok(()) => {
                if self.terminal {
                    TaskOutcome::Terminate(Box::new(NoopTerminalTask))
                } else {
                    TaskOutcome::Done
                }
            }
            Err(e) => {
                error!("Failed to send HTTP reply: {:?}", e);
                TaskOutcome::Failed
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kuadrant::MockWasmHost;
    use std::sync::Arc;

    #[test]
    fn test_send_reply_task_success() {
        let mock_host = MockWasmHost::new();
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));

        let predicate = Predicate::new("true").unwrap();
        let deny_with = Expression::new(
            "DenyResponse { status: 403u, headers: [['content-type', 'text/plain'], ['WWW-Authenticate', 'APIKEY realm=\"api-key-users\"']], body: 'Access Denied' }"
        ).unwrap();
        let task = Box::new(SendReplyTask::new(
            "0".to_string(),
            predicate,
            deny_with,
            false,
        ));

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Done));
    }

    #[test]
    fn test_send_reply_task_no_body() {
        let mock_host = MockWasmHost::new();
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));

        let predicate = Predicate::new("true").unwrap();
        let deny_with =
            Expression::new("DenyResponse { status: 429u, headers: [], body: '' }").unwrap();
        let task = Box::new(SendReplyTask::new(
            "0".to_string(),
            predicate,
            deny_with,
            false,
        ));

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Done));
    }
}
