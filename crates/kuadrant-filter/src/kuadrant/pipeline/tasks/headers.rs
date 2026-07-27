use crate::data::attribute::{AttributeState, Path};
use crate::data::cel::Predicate;
use crate::data::{Expression, Headers};
use crate::kuadrant::pipeline::tasks::{SendReplyTask, Task, TaskOutcome};
use crate::kuadrant::ReqRespCtx;
use crate::services::cel_value_to_header_pairs;
use tracing::{debug, error};

#[derive(Clone, Debug)]
pub enum HeadersType {
    HttpRequestHeaders,
    HttpResponseHeaders,
}

#[derive(Clone)]
#[allow(dead_code)]
pub enum HeaderOperation {
    Append(Headers),
    Set(Headers),
    Remove(Vec<String>),
}

impl From<&HeadersType> for Path {
    fn from(header_type: &HeadersType) -> Self {
        match header_type {
            HeadersType::HttpRequestHeaders => Path::new(vec!["request", "headers"]),
            HeadersType::HttpResponseHeaders => Path::new(vec!["response", "headers"]),
        }
    }
}

#[derive(Clone)]
pub struct ModifyHeadersTask {
    task_id: String,
    predicate: Predicate,
    headers_expr: Expression,
    target: HeadersType,
    terminal: bool,
}

impl ModifyHeadersTask {
    pub fn new(
        task_id: String,
        predicate: Predicate,
        headers_expr: Expression,
        target: HeadersType,
        terminal: bool,
    ) -> Self {
        Self {
            task_id,
            predicate,
            headers_expr,
            target,
            terminal,
        }
    }
}

impl Task for ModifyHeadersTask {
    fn id(&self) -> &str {
        &self.task_id
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

        let operation = match self.headers_expr.eval(ctx, &mut cel_ctx) {
            Ok(AttributeState::Pending) => {
                return TaskOutcome::Requeued(vec![self]);
            }
            Ok(AttributeState::Available(ref val)) => {
                let pairs = cel_value_to_header_pairs(val);
                if pairs.is_empty() {
                    return TaskOutcome::Done;
                }
                HeaderOperation::Set(pairs.into())
            }
            Err(e) => {
                error!("Failed to evaluate headers expression: {e}");
                return TaskOutcome::Failed;
            }
        };

        let path: Path = (&self.target).into();
        let result: Result<AttributeState<Option<Headers>>, _> = ctx.get_attribute_ref(&path);
        match result {
            Ok(AttributeState::Available(Some(mut existing_headers))) => {
                let _span = tracing::debug_span!("headers", target = ?self.target).entered();

                match &operation {
                    HeaderOperation::Append(headers) => {
                        debug!("Appending {} headers", headers.len());
                        existing_headers.extend(headers.clone());
                    }
                    HeaderOperation::Set(headers) => {
                        debug!("Setting {} headers", headers.len());
                        for (key, value) in headers.clone().into_inner() {
                            existing_headers.set(key, value);
                        }
                    }
                    HeaderOperation::Remove(keys) => {
                        debug!("Removing {} headers", keys.len());
                        for key in keys {
                            existing_headers.remove(key);
                        }
                    }
                }
                match ctx.set_attribute_map(&path, existing_headers) {
                    Ok(AttributeState::Available(_)) => {
                        if self.terminal {
                            TaskOutcome::Terminate(Box::new(SendReplyTask::default()))
                        } else {
                            TaskOutcome::Done
                        }
                    }
                    Ok(AttributeState::Pending) => TaskOutcome::Requeued(vec![self]),
                    Err(e) => {
                        error!("Failed to set attribute map: {e:?}");
                        TaskOutcome::Failed
                    }
                }
            }
            Ok(AttributeState::Available(None)) => {
                let _span = tracing::debug_span!("headers", target = ?self.target).entered();
                error!(
                    "Unexpected state: getting headers returned AttributeState::Available(None)"
                );
                TaskOutcome::Failed
            }
            Ok(AttributeState::Pending) => TaskOutcome::Requeued(vec![self]),
            Err(e) => {
                let _span = tracing::debug_span!("headers", target = ?self.target).entered();
                error!("Failed to get attribute reference: {e:?}");
                TaskOutcome::Failed
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::attribute::Path;
    use crate::data::cel::Predicate;
    use crate::data::Expression;
    use crate::kuadrant::MockWasmHost;
    use std::sync::Arc;

    #[test]
    fn append_headers_task() {
        let existing_headers = vec![("API-Key".to_string(), "API-Value".to_string())];
        let mock_host =
            MockWasmHost::new().with_map("request.headers".to_string(), existing_headers);
        let backend = Arc::new(mock_host);
        let mut ctx = ReqRespCtx::new(backend);

        let predicate = Predicate::new("true").unwrap();
        let headers_expr = Expression::new("[['New-Key', 'New-Value']]").unwrap();

        let task = Box::new(ModifyHeadersTask::new(
            "0".to_string(),
            predicate,
            headers_expr,
            HeadersType::HttpRequestHeaders,
            false,
        ));

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Done));

        let result: Result<AttributeState<Option<Headers>>, _> =
            ctx.get_attribute_ref(&Path::from(&HeadersType::HttpRequestHeaders));

        assert!(matches!(result, Ok(AttributeState::Available(Some(_)))));
        if let Ok(AttributeState::Available(Some(headers))) = result {
            assert_eq!(headers.len(), 2);
            assert_eq!(headers.get("API-Key"), Some("API-Value"));
            assert_eq!(headers.get("New-Key"), Some("New-Value"));
        }
    }

    #[test]
    fn set_headers_task() {
        let existing_headers = vec![
            ("Content-Type".to_string(), "text/plain".to_string()),
            ("X-Custom".to_string(), "value1".to_string()),
        ];
        let mock_host =
            MockWasmHost::new().with_map("request.headers".to_string(), existing_headers);
        let backend = Arc::new(mock_host);
        let mut ctx = ReqRespCtx::new(backend);

        let predicate = Predicate::new("true").unwrap();
        let headers_expr = Expression::new("[['Content-Type', 'application/json']]").unwrap();

        let task = Box::new(ModifyHeadersTask::new(
            "0".to_string(),
            predicate,
            headers_expr,
            HeadersType::HttpRequestHeaders,
            false,
        ));

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Done));

        let result: Result<AttributeState<Option<Headers>>, _> =
            ctx.get_attribute_ref(&Path::from(&HeadersType::HttpRequestHeaders));

        assert!(matches!(result, Ok(AttributeState::Available(Some(_)))));
        if let Ok(AttributeState::Available(Some(headers))) = result {
            assert_eq!(headers.len(), 2);
            assert_eq!(headers.get("Content-Type"), Some("application/json"));
            assert_eq!(headers.get("X-Custom"), Some("value1"));
        }
    }

    #[test]
    fn empty_headers_expr_returns_done() {
        let existing_headers = vec![
            ("API-Key-To-Remove".to_string(), "API-Value".to_string()),
            ("X-Origin".to_string(), "Kuadrant".to_string()),
        ];
        let mock_host =
            MockWasmHost::new().with_map("response.headers".to_string(), existing_headers);
        let backend = Arc::new(mock_host);
        let mut ctx = ReqRespCtx::new(backend);

        let predicate = Predicate::new("true").unwrap();
        let headers_expr = Expression::new("[]").unwrap();

        let task = Box::new(ModifyHeadersTask::new(
            "0".to_string(),
            predicate,
            headers_expr,
            HeadersType::HttpResponseHeaders,
            false,
        ));

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Done));
    }
}
