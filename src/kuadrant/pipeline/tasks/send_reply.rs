use cel::common::types::{CelString, CelUInt};
use cel::Value;
use tracing::error;

use crate::data::attribute::AttributeState;
use crate::data::cel::Predicate;
use crate::data::Expression;
use crate::kuadrant::pipeline::tasks::{NoopTerminalTask, Task, TaskOutcome};
use crate::kuadrant::ReqRespCtx;
use crate::metrics::METRICS;
use crate::services::{cel_value_to_header_pairs, deny_response_struct_def};

pub struct SendReplyTask {
    task_id: String,
    predicate: Option<Predicate>,
    deny_with: Expression,
    terminal: bool,
}

impl SendReplyTask {
    pub fn new(
        task_id: String,
        status_code: u32,
        headers: Vec<(String, String)>,
        body: Option<String>,
    ) -> Self {
        let headers = headers
            .into_iter()
            .map(|(h, v)| format!("['''{h}''', '''{v}''']"))
            .collect::<Vec<String>>()
            .join(", ");
        let body_field = body.map(|b| format!("body: '''{b}'''")).unwrap_or_default();
        let expr = format!(
            "DenyResponse {{ status: {status_code}u, headers: [{headers}], {body_field} }}"
        );
        #[allow(clippy::expect_used)]
        let deny_with = Expression::new(&expr).expect("Needs to be valid CEL!");
        Self {
            task_id,
            predicate: None,
            deny_with,
            terminal: false,
        }
    }

    pub fn new_deferred(
        task_id: String,
        predicate: Predicate,
        deny_with: Expression,
        terminal: bool,
    ) -> Self {
        Self {
            task_id,
            predicate: Some(predicate),
            deny_with,
            terminal,
        }
    }

    pub fn default() -> Self {
        Self::new(
            "default".to_string(),
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

        Ok(Self::new("from_value".to_string(), status, headers, body))
    }
}

impl Task for SendReplyTask {
    fn id(&self) -> &str {
        &self.task_id
    }

    fn cel_types(&self) -> Vec<cel::StructDef> {
        vec![deny_response_struct_def()]
    }

    #[tracing::instrument(name = "send_reply", skip(self, ctx))]
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        if let Some(ref predicate) = self.predicate {
            let mut cel_ctx = ctx.cel.new_ctx(&*self);
            match predicate.test(ctx, &mut cel_ctx) {
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

        let (status_code, headers, body) = {
            let mut cel_ctx = ctx.cel.new_ctx(&*self);
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
            "0".to_string(),
            403,
            vec![
                ("content-type".to_string(), "text/plain".to_string()),
                (
                    "WWW-Authenticate".to_string(),
                    "APIKEY realm=\"api-key-users\"".to_string(),
                ),
            ],
            Some("Access Denied".to_string()),
        ));

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Done));
    }

    #[test]
    fn test_send_reply_task_no_body() {
        let mock_host = MockWasmHost::new();
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));

        let task = Box::new(SendReplyTask::new("0".to_string(), 429, vec![], None));

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Done));
    }
}
