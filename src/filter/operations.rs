use crate::service::{DirectResponse, Headers, IndexedGrpcRequest};

#[derive(Debug)]
pub enum ProcessGrpcMessageOperation {
    EventualOps(Vec<EventualOperation>),
    DirectResponse(DirectResponse),
}

impl From<DirectResponse> for ProcessGrpcMessageOperation {
    fn from(e: DirectResponse) -> Self {
        ProcessGrpcMessageOperation::DirectResponse(e)
    }
}

impl From<Vec<EventualOperation>> for ProcessGrpcMessageOperation {
    fn from(e: Vec<EventualOperation>) -> Self {
        ProcessGrpcMessageOperation::EventualOps(e)
    }
}

#[derive(Debug)]
pub enum EventualOperation {
    AddRequestHeaders(Headers),
    AddResponseHeaders(Headers),
}

pub enum ProcessNextRequestOperation {
    GrpcRequest(IndexedGrpcRequest),
    AwaitRequestBody(usize),
    AwaitResponseBody(usize),
    Done,
}

impl From<IndexedGrpcRequest> for ProcessNextRequestOperation {
    fn from(e: IndexedGrpcRequest) -> Self {
        ProcessNextRequestOperation::GrpcRequest(e)
    }
}
