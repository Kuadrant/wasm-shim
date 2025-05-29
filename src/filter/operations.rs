use crate::service::{DirectResponse, Headers};

#[derive(Debug)]
pub enum Operation {
    EventualOps(Vec<EventualOperation>),
    DirectResponse(DirectResponse),
}

impl From<DirectResponse> for Operation {
    fn from(e: DirectResponse) -> Self {
        Operation::DirectResponse(e)
    }
}

impl From<Vec<EventualOperation>> for Operation {
    fn from(e: Vec<EventualOperation>) -> Self {
        Operation::EventualOps(e)
    }
}

#[derive(Debug)]
pub enum EventualOperation {
    AddRequestHeaders(Headers),
    AddResponseHeaders(Headers),
}
