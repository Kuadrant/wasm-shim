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

            if ctx.set_attribute_map(&path, new_headers).is_ok() {
                TaskOutcome::Done
            } else {
                TaskOutcome::Pending(self)
            }
        } else {
            TaskOutcome::Pending(self)
        }
    }
}
