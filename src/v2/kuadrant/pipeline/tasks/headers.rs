#[allow(dead_code)]
use crate::v2::data::attribute::{AttributeState, Path};
use crate::v2::data::Headers;
use crate::v2::kuadrant::pipeline::tasks::{Task, TaskOutcome};
use crate::v2::kuadrant::ReqRespCtx;

#[derive(Clone)]
pub enum HeadersType {
    HttpRequestHeaders,
    HttpResponseHeaders,
}

#[derive(Clone)]
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
struct ModifyHeadersTask {
    operation: HeaderOperation,
    target: HeadersType,
}

impl ModifyHeadersTask {
    pub fn new(operation: HeaderOperation, target: HeadersType) -> ModifyHeadersTask {
        ModifyHeadersTask { operation, target }
    }
}

impl Task for ModifyHeadersTask {
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        let path: Path = (&self.target).into();
        let result: Result<AttributeState<Option<Headers>>, _> = ctx.get_attribute_ref(&path);
        match result {
            Ok(AttributeState::Available(Some(mut existing_headers))) => {
                match &self.operation {
                    HeaderOperation::Append(headers) => {
                        existing_headers.extend(headers.clone());
                    }
                    HeaderOperation::Set(headers) => {
                        for (key, value) in headers.clone().into_inner() {
                            existing_headers.set(key, value);
                        }
                    }
                    HeaderOperation::Remove(keys) => {
                        for key in keys {
                            existing_headers.remove(key);
                        }
                    }
                }
                match ctx.set_attribute_map(&path, existing_headers) {
                    Ok(AttributeState::Available(_)) => TaskOutcome::Done,
                    Ok(AttributeState::Pending) => TaskOutcome::Requeued(vec![self]),
                    Err(_) => TaskOutcome::Failed,
                }
            }
            Ok(AttributeState::Available(None)) => {
                unreachable!("get_attribute_ref can't return AttributeState::Available(None)")
            }
            Ok(AttributeState::Pending) => TaskOutcome::Requeued(vec![self]),
            Err(_) => {
                // TODO: Error handling since this was a major failure.
                TaskOutcome::Failed
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::kuadrant::MockWasmHost;
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
            HeaderOperation::Append(new_headers),
            HeadersType::HttpRequestHeaders,
        ));

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Done));

        let result: Result<AttributeState<Option<Headers>>, _> =
            ctx.get_attribute_ref(&Path::from(&HeadersType::HttpRequestHeaders));

        if let Ok(AttributeState::Available(Some(headers))) = result {
            assert_eq!(headers.len(), 2);
            assert_eq!(headers.get("API-Key"), Some("API-Value"));
            assert_eq!(headers.get("New-Key"), Some("New-Value"));
        } else {
            unreachable!("Expected AttributeState::Available(Some(headers))");
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
            HeaderOperation::Set(new_headers),
            HeadersType::HttpRequestHeaders,
        ));

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Done));

        let result: Result<AttributeState<Option<Headers>>, _> =
            ctx.get_attribute_ref(&Path::from(&HeadersType::HttpRequestHeaders));

        if let Ok(AttributeState::Available(Some(headers))) = result {
            assert_eq!(headers.len(), 2);
            assert_eq!(headers.get("Content-Type"), Some("application/json"));
            assert_eq!(headers.get("X-Custom"), Some("value1"));
        } else {
            unreachable!("Expected AttributeState::Available(Some(headers))");
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
            HeaderOperation::Remove(keys_to_remove),
            HeadersType::HttpResponseHeaders,
        ));

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Done));

        let result: Result<AttributeState<Option<Headers>>, _> =
            ctx.get_attribute_ref(&Path::from(&HeadersType::HttpResponseHeaders));

        if let Ok(AttributeState::Available(Some(headers))) = result {
            assert_eq!(headers.len(), 1);
            assert_eq!(headers.get("X-Origin"), Some("Kuadrant"));
        } else {
            unreachable!("Expected AttributeState::Available(Some(headers))");
        }
    }
}
