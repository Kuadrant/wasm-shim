use crate::envoy::StatusCode;
use crate::kuadrant::pipeline::tasks::{Task, TaskOutcome};
use crate::kuadrant::ReqRespCtx;
use log::error;

pub struct SendReplyTask {
    status_code: u32,
    headers: Vec<(String, String)>,
    body: Option<String>,
}

impl SendReplyTask {
    pub fn new(status_code: u32, headers: Vec<(String, String)>, body: Option<String>) -> Self {
        Self {
            status_code,
            headers,
            body,
        }
    }

    pub fn default() -> Self {
        Self::new(
            StatusCode::InternalServerError as u32,
            Vec::new(),
            Some("Internal Server Error.\n".to_string()),
        )
    }
}

impl Task for SendReplyTask {
    #[tracing::instrument(name = "send_reply", skip(self, ctx), fields(status_code = %self.status_code))]
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        let headers_ref: Vec<(&str, &str)> = self
            .headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

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
