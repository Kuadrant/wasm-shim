use crate::filter::operations::Operation::SendGrpcRequest;
use crate::runtime_action_set::RuntimeActionSet;
use crate::service::{GrpcErrResponse, GrpcRequest, IndexedGrpcRequest};
use std::rc::Rc;

pub enum Operation {
    SendGrpcRequest(GrpcMessageSenderOperation),
    AwaitGrpcResponse(GrpcMessageReceiverOperation),
    AddHeaders(HeadersOperation),
    Die(GrpcErrResponse),
    // Done indicates that we have no more operations and can resume the http request flow
    Done(),
}

pub struct GrpcMessageSenderOperation {
    runtime_action_set: Rc<RuntimeActionSet>,
    grpc_request: IndexedGrpcRequest,
}

impl GrpcMessageSenderOperation {
    pub fn new(
        runtime_action_set: Rc<RuntimeActionSet>,
        indexed_request: IndexedGrpcRequest,
    ) -> Self {
        Self {
            runtime_action_set,
            grpc_request: indexed_request,
        }
    }

    pub fn build_receiver_operation(self) -> (GrpcRequest, Operation) {
        let index = self.grpc_request.index();
        (
            self.grpc_request.request(),
            Operation::AwaitGrpcResponse(GrpcMessageReceiverOperation::new(
                self.runtime_action_set,
                index,
            )),
        )
    }
}

pub struct GrpcMessageReceiverOperation {
    runtime_action_set: Rc<RuntimeActionSet>,
    current_index: usize,
}

impl GrpcMessageReceiverOperation {
    pub fn new(runtime_action_set: Rc<RuntimeActionSet>, current_index: usize) -> Self {
        Self {
            runtime_action_set,
            current_index,
        }
    }

    pub fn digest_grpc_response(self, msg: &[u8]) -> Vec<Operation> {
        let result = self
            .runtime_action_set
            .process_grpc_response(self.current_index, msg);

        match result {
            Ok((next_msg, headers)) => {
                let mut operations = Vec::new();
                if !headers.is_empty() {
                    operations.push(Operation::AddHeaders(HeadersOperation::new(headers)))
                }
                operations.push(match next_msg {
                    None => Operation::Done(),
                    Some(indexed_req) => SendGrpcRequest(GrpcMessageSenderOperation::new(
                        self.runtime_action_set,
                        indexed_req,
                    )),
                });
                operations
            }
            Err(grpc_err_resp) => vec![Operation::Die(grpc_err_resp)],
        }
    }

    pub fn fail(self) -> Operation {
        //todo(adam-cattermole): should this take into account failure mode?
        // these errors occurred at filter layer,
        // i.e. error response / failed to read buffer / failed serdes
        Operation::Die(GrpcErrResponse::new_internal_server_error())
    }
}

pub struct HeadersOperation {
    headers: Vec<(String, String)>,
}

impl HeadersOperation {
    pub fn new(headers: Vec<(String, String)>) -> Self {
        Self { headers }
    }

    pub fn headers(self) -> Vec<(String, String)> {
        self.headers
    }
}
