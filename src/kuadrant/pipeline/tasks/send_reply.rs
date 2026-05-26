use cel::common::types::{CelString, CelUInt};
use cel::Value;
use tracing::error;

use crate::data::attribute::AttributeState;
use crate::data::cel::Predicate;
use crate::data::Expression;
use crate::kuadrant::pipeline::tasks::{NoopTerminalTask, Task, TaskOutcome};
use crate::kuadrant::ReqRespCtx;
use crate::metrics::METRICS;
use crate::services::cel_value_to_header_pairs;

enum SendReplyMode {
    Concrete {
        status_code: u32,
        headers: Vec<(String, String)>,
        body: Option<String>,
    },
    Deferred {
        deny_with: Expression,
    },
}

pub struct SendReplyTask {
    predicate: Option<Predicate>,
    mode: SendReplyMode,
    terminal: bool,
}

impl SendReplyTask {
    pub fn new(status_code: u32, headers: Vec<(String, String)>, body: Option<String>) -> Self {
        Self {
            predicate: None,
            mode: SendReplyMode::Concrete {
                status_code,
                headers,
                body,
            },
            terminal: false,
        }
    }

    pub fn new_deferred(predicate: Predicate, deny_with: Expression, terminal: bool) -> Self {
        Self {
            predicate: Some(predicate),
            mode: SendReplyMode::Deferred { deny_with },
            terminal,
        }
    }

    pub fn default() -> Self {
        Self::new(
            500,
            Vec::new(),
            Some("Internal Server Error.\n".to_string()),
        )
    }
}

impl TryFrom<Value> for SendReplyTask {
    type Error = String;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        let Value::Struct(deny_response) = value else {
            return Err(format!("expected DenyResponse struct, got: {value:?}"));
        };

        let status = deny_response
            .field_value("status")
            .and_then(|v| v.downcast_ref::<CelUInt>())
            .map(|v| *v.inner() as u32)
            .ok_or("DenyResponse missing or invalid 'status' field")?;

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

        Ok(Self::new(status, headers, body))
    }
}

impl Task for SendReplyTask {
    #[tracing::instrument(name = "send_reply", skip(self, ctx))]
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        if let Some(ref predicate) = self.predicate {
            match predicate.test(ctx) {
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
        }

        let (status_code, headers, body) = match &self.mode {
            SendReplyMode::Concrete {
                status_code,
                headers,
                body,
            } => (*status_code, headers.clone(), body.clone()),
            SendReplyMode::Deferred { deny_with } => {
                let mut cel_ctx = cel::Context::default();
                match deny_with.eval(ctx, &mut cel_ctx) {
                    Ok(AttributeState::Pending) => {
                        error!("Unexpected pending state in deny expression");
                        return TaskOutcome::Failed;
                    }
                    Ok(AttributeState::Available(val @ Value::Struct(_))) => {
                        match SendReplyTask::try_from(val) {
                            Ok(concrete_task) => {
                                if let SendReplyMode::Concrete {
                                    status_code,
                                    headers,
                                    body,
                                } = concrete_task.mode
                                {
                                    (status_code, headers, body)
                                } else {
                                    error!("Expected concrete task from try_from");
                                    return TaskOutcome::Failed;
                                }
                            }
                            Err(e) => {
                                error!("Invalid DenyResponse: {e}");
                                return TaskOutcome::Failed;
                            }
                        }
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
            }
        };

        let mut headers_ref: Vec<(&str, &str)> = headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        if let Some((tracker, value)) = ctx.tracker() {
            headers_ref.push((tracker, value));
        }

        METRICS.denied().increment();

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

        let task = Box::new(SendReplyTask::new(
            403,
            vec![("content-type".to_string(), "text/plain".to_string())],
            Some("Access Denied".to_string()),
        ));

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Done));
    }

    #[test]
    fn test_send_reply_task_no_body() {
        let mock_host = MockWasmHost::new();
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));

        let task = Box::new(SendReplyTask::new(429, vec![], None));

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Done));
    }
}
