use cel::common::types::{CelString, CelUInt};
use cel::Value;
use tracing::error;

use crate::kuadrant::pipeline::tasks::{Task, TaskOutcome};
use crate::kuadrant::ReqRespCtx;
use crate::metrics::METRICS;
use crate::services::cel_value_to_header_pairs;

pub struct SendReplyTask {
    status_code: u32,
    headers: Vec<(String, String)>,
    body: Option<String>,
}

impl SendReplyTask {
    pub fn new(status_code: u32, headers: Vec<(String, String)>, body: Option<String>) -> Self {
        METRICS.denied().increment();
        Self {
            status_code,
            headers,
            body,
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
    #[tracing::instrument(name = "send_reply", skip(self, ctx), fields(status_code = %self.status_code))]
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        let mut headers_ref: Vec<(&str, &str)> = self
            .headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        if let Some((tracker, value)) = ctx.tracker() {
            headers_ref.push((tracker, value));
        }

        let body_bytes = self.body.as_ref().map(|s| s.as_bytes());
        match ctx.send_http_reply(self.status_code, headers_ref, body_bytes) {
            Ok(()) => TaskOutcome::Done,
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
