use crate::configuration::FailureMode;
use crate::runtime_action::RuntimeAction;
use crate::runtime_action_set::{IndexedRequestResult, RuntimeActionSet};
use crate::service::{GrpcErrResponse, GrpcRequest, HeaderKind, IndexedGrpcRequest};
use std::rc::Rc;

pub enum Operation {
    SendGrpcRequest(GrpcMessageSenderOperation),
    AwaitGrpcResponse(GrpcMessageReceiverOperation),
    AddHeaders(HeaderOperation),
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

    pub fn build_receiver_operation(self) -> (GrpcRequest, GrpcMessageReceiverOperation) {
        let index = self.grpc_request.index();
        (
            self.grpc_request.request(),
            GrpcMessageReceiverOperation::new(self.runtime_action_set, index),
        )
    }

    pub fn get_failure_mode(&self) -> FailureMode {
        self.runtime_action_set.runtime_actions[self.grpc_request.index()].get_failure_mode()
    }

    pub fn get_runtime_action(&self) -> &Rc<RuntimeAction> {
        &self.runtime_action_set.runtime_actions[self.grpc_request.index()]
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
                    operations.push(Operation::AddHeaders(HeaderOperation::new(headers)))
                }
                operations.push(self.handle_next(next_msg));
                operations
            }
            Err(grpc_err_resp) => vec![Operation::Die(grpc_err_resp)],
        }
    }

    pub fn fail(self) -> Operation {
        match self.runtime_action_set.runtime_actions[self.current_index].get_failure_mode() {
            FailureMode::Deny => Operation::Die(GrpcErrResponse::new_internal_server_error()),
            FailureMode::Allow => {
                let next = self
                    .runtime_action_set
                    .find_next_grpc_request(self.current_index + 1);
                self.handle_next(next)
            }
        }
    }

    fn handle_next(self, indexed_request_result: IndexedRequestResult) -> Operation {
        match indexed_request_result {
            Ok(None) => Operation::Done(),
            Ok(Some(indexed_req)) => Operation::SendGrpcRequest(GrpcMessageSenderOperation::new(
                self.runtime_action_set,
                indexed_req,
            )),
            Err(grpc_err_resp) => Operation::Die(grpc_err_resp),
        }
    }
}

pub struct HeaderOperation {
    headers: HeaderKind,
}

impl HeaderOperation {
    pub fn new(headers: HeaderKind) -> Self {
        Self { headers }
    }

    pub fn into_inner(self) -> HeaderKind {
        self.headers
    }
}
