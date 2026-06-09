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

enum HeadersMode {
    Concrete { operation: HeaderOperation },
    Deferred { headers_expr: Expression },
}

#[derive(Clone)]
pub struct ModifyHeadersTask {
    task_id: String,
    predicate: Option<Predicate>,
    mode: HeadersMode,
    target: HeadersType,
    terminal: bool,
}

impl Clone for HeadersMode {
    fn clone(&self) -> Self {
        match self {
            HeadersMode::Concrete { operation } => HeadersMode::Concrete {
                operation: operation.clone(),
            },
            HeadersMode::Deferred { headers_expr } => HeadersMode::Deferred {
                headers_expr: headers_expr.clone(),
            },
        }
    }
}

impl ModifyHeadersTask {
    pub fn new(
        task_id: String,
        operation: HeaderOperation,
        target: HeadersType,
    ) -> ModifyHeadersTask {
        ModifyHeadersTask {
            task_id,
            predicate: None,
            mode: HeadersMode::Concrete { operation },
            target,
            terminal: false,
        }
    }

    pub fn new_deferred(
        task_id: String,
        predicate: Predicate,
        headers_expr: Expression,
        target: HeadersType,
        terminal: bool,
    ) -> Self {
        Self {
            task_id,
            predicate: Some(predicate),
            mode: HeadersMode::Deferred { headers_expr },
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

        let operation = match &self.mode {
            HeadersMode::Concrete { operation } => operation.clone(),
            HeadersMode::Deferred { headers_expr } => {
                let mut cel_ctx = ctx.cel.new_ctx(&*self);
                match headers_expr.eval(ctx, &mut cel_ctx) {
                    Ok(AttributeState::Pending) => {
                        error!("Unexpected pending state in headers expression");
                        return TaskOutcome::Failed;
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
                }
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
    use crate::kuadrant::MockWasmHost;
    use std::sync::Arc;

    #[test]
    fn append_headers_task() {
        let existing_headers = vec![("API-Key".to_string(), "API-Value".to_string())];
        let mock_host =
            MockWasmHost::new().with_map("request.headers".to_string(), existing_headers);
        let backend = Arc::new(mock_host);
        let mut ctx = ReqRespCtx::new(backend);

        let new_headers: Headers = vec![("New-Key".to_string(), "New-Value".to_string())].into();

        let task = Box::new(ModifyHeadersTask::new(
            "0".to_string(),
            HeaderOperation::Append(new_headers),
            HeadersType::HttpRequestHeaders,
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

        let new_headers: Headers =
            vec![("Content-Type".to_string(), "application/json".to_string())].into();

        let task = Box::new(ModifyHeadersTask::new(
            "0".to_string(),
            HeaderOperation::Set(new_headers),
            HeadersType::HttpRequestHeaders,
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
    fn remove_headers_task() {
        let existing_headers = vec![
            ("API-Key-To-Remove".to_string(), "API-Value".to_string()),
            ("X-Origin".to_string(), "Kuadrant".to_string()),
        ];
        let mock_host =
            MockWasmHost::new().with_map("response.headers".to_string(), existing_headers);
        let backend = Arc::new(mock_host);
        let mut ctx = ReqRespCtx::new(backend);

        let keys_to_remove = vec!["API-Key-To-Remove".to_string()];

        let task = Box::new(ModifyHeadersTask::new(
            "0".to_string(),
            HeaderOperation::Remove(keys_to_remove),
            HeadersType::HttpResponseHeaders,
        ));

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Done));

        let result: Result<AttributeState<Option<Headers>>, _> =
            ctx.get_attribute_ref(&Path::from(&HeadersType::HttpResponseHeaders));

        assert!(matches!(result, Ok(AttributeState::Available(Some(_)))));
        if let Ok(AttributeState::Available(Some(headers))) = result {
            assert_eq!(headers.len(), 1);
            assert_eq!(headers.get("X-Origin"), Some("Kuadrant"));
        }
    }
}
