use crate::v2::data::attribute::{AttributeError, AttributeState, Path};
use crate::v2::kuadrant::ReqRespCtx;
use crate::v2::tasks::{Task, TaskOutcome};
use std::collections::HashMap;

#[derive(Clone)]
pub enum HeadersType {
    HttpRequestHeaders,
    HttpResponseHeaders,
}

#[derive(Clone)]
pub enum HeadersAction {
    Add,
    Remove,
    Update,
}

impl From<HeadersType> for Path {
    fn from(header_type: HeadersType) -> Self {
        match header_type {
            HeadersType::HttpRequestHeaders => Path::new(vec!["request", "headers"]),
            HeadersType::HttpResponseHeaders => Path::new(vec!["response", "headers"]),
        }
    }
}

#[derive(Clone)]
struct HandleHeadersTask {
    headers: HashMap<String, String>,
    headers_type: HeadersType,
    headers_action: HeadersAction,
}

impl HandleHeadersTask {
    pub fn new(
        headers: HashMap<String, String>,
        headers_type: HeadersType,
        headers_action: HeadersAction,
    ) -> HandleHeadersTask {
        HandleHeadersTask {
            headers,
            headers_type,
            headers_action,
        }
    }
}

impl Task for HandleHeadersTask {
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        let path: Path = self.headers_type.clone().into();
        let result: Result<AttributeState<Option<HashMap<String, String>>>, _> =
            ctx.get_attribute_ref(&path);
        match result {
            Ok(AttributeState::Available(Some(cached_headers))) => {
                let mut new_headers = HashMap::<String, String>::new();
                new_headers.extend(cached_headers);
                match self.headers_action {
                    HeadersAction::Add | HeadersAction::Update => {
                        // TODO: We could merge the value when adding, now treating as Update
                        new_headers.extend(self.headers.clone());
                    }
                    HeadersAction::Remove => {
                        new_headers.retain(|k, _| !self.headers.contains_key(k));
                    }
                }
                if ctx.set_attribute_map(&path, new_headers).is_ok() {
                    TaskOutcome::Done
                } else {
                    TaskOutcome::Pending(self)
                }
            }
            Ok(AttributeState::Available(None)) => {
                unreachable!("get_attribute_ref can't return AttributeState::Available(None)")
            }
            Ok(AttributeState::Pending) => TaskOutcome::Pending(self),
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
    fn add_headers_task() {
        let mut existing_headers = HashMap::new();
        existing_headers.insert("API-Key".to_string(), "API-Value".to_string());
        let mock_host =
            MockWasmHost::new().with_map("request.headers".to_string(), existing_headers);
        let backend = Arc::new(mock_host);
        let mut ctx = ReqRespCtx::new(backend);

        let mut new_headers = HashMap::new();
        new_headers.insert("New-Key".to_string(), "New-Value".to_string());

        let task = Box::new(HandleHeadersTask::new(
            new_headers,
            HeadersType::HttpRequestHeaders,
            HeadersAction::Add,
        ));

        match task.apply(&mut ctx) {
            TaskOutcome::Done => (),
            _ => panic!("Expected TaskOutcome::Done"),
        }

        let result: Result<AttributeState<Option<HashMap<String, String>>>, _> =
            ctx.get_attribute_ref(&Path::from(HeadersType::HttpRequestHeaders));
        match result {
            Ok(AttributeState::Available(Some(headers))) => {
                assert_eq!(headers.len(), 2);
                assert_eq!(headers["API-Key"], "API-Value");
                assert_eq!(headers["New-Key"], "New-Value");
            }
            _ => panic!("Expected AttributeState::Available(Some(headers))"),
        }
    }

    #[test]
    fn update_headers_task() {
        let mut existing_headers = HashMap::new();
        existing_headers.insert("API-Key".to_string(), "API-Value".to_string());
        let mock_host =
            MockWasmHost::new().with_map("request.headers".to_string(), existing_headers);
        let backend = Arc::new(mock_host);
        let mut ctx = ReqRespCtx::new(backend);

        let mut new_headers = HashMap::new();
        new_headers.insert("API-Key".to_string(), "New-Value".to_string());

        let task = Box::new(HandleHeadersTask::new(
            new_headers,
            HeadersType::HttpRequestHeaders,
            HeadersAction::Update,
        ));

        match task.apply(&mut ctx) {
            TaskOutcome::Done => (),
            _ => panic!("Expected TaskOutcome::Done"),
        }

        let result: Result<AttributeState<Option<HashMap<String, String>>>, _> =
            ctx.get_attribute_ref(&Path::from(HeadersType::HttpRequestHeaders));
        match result {
            Ok(AttributeState::Available(Some(headers))) => {
                assert_eq!(headers.len(), 1);
                assert_eq!(headers["API-Key"], "New-Value");
            }
            _ => panic!("Expected AttributeState::Available(Some(headers))"),
        }
    }

    #[test]
    fn remove_headers_task() {
        let mut existing_headers = HashMap::new();
        existing_headers.insert("API-Key-To-Remove".to_string(), "API-Value".to_string());
        existing_headers.insert("X-Origin".to_string(), "Kuadrant".to_string());
        let mock_host =
            MockWasmHost::new().with_map("response.headers".to_string(), existing_headers);
        let backend = Arc::new(mock_host);
        let mut ctx = ReqRespCtx::new(backend);

        let mut headers_to_remove = HashMap::new();
        headers_to_remove.insert("API-Key-To-Remove".to_string(), "".to_string());

        let task = Box::new(HandleHeadersTask::new(
            headers_to_remove,
            HeadersType::HttpResponseHeaders,
            HeadersAction::Remove,
        ));

        match task.apply(&mut ctx) {
            TaskOutcome::Done => (),
            _ => panic!("Expected TaskOutcome::Done"),
        }

        let result: Result<AttributeState<Option<HashMap<String, String>>>, _> =
            ctx.get_attribute_ref(&Path::from(HeadersType::HttpResponseHeaders));
        match result {
            Ok(AttributeState::Available(Some(headers))) => {
                assert_eq!(headers.len(), 1);
                assert_eq!(headers["X-Origin"], "Kuadrant");
            }
            _ => panic!("Expected AttributeState::Available(Some(headers))"),
        }
    }
}
