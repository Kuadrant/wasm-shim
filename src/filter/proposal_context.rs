use crate::action_set_index::ActionSetIndex;
use crate::filter::proposal_context::no_implicit_dep::{
    GrpcMessageReceiverOperation, HeadersOperation, Operation,
};
use crate::runtime_action_set::RuntimeActionSet;
use crate::service::{GrpcErrResponse, GrpcRequestAction, HeaderResolver};
use log::{debug, warn};
use proxy_wasm::traits::{Context, HttpContext};
use proxy_wasm::types::{Action, Status};
use std::mem;
use std::rc::Rc;

pub mod no_implicit_dep {
    use crate::runtime_action_set::RuntimeActionSet;
    use crate::service::{GrpcErrResponse, GrpcRequestAction};
    use std::rc::Rc;

    #[allow(dead_code)]
    pub enum Operation {
        AwaitGrpcResponse(GrpcMessageReceiverOperation),
        AddHeaders(HeadersOperation),
        Die(GrpcErrResponse),
        //todo(adam-cattermole): does Done make sense? in this case no PendingOperation
        // instead just Option<PendingOperation>?
        Done(),
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

        pub fn digest_grpc_response(
            self,
            msg: &[u8],
        ) -> Result<(Option<GrpcRequestAction>, Option<Operation>), Operation> {
            let result = self
                .runtime_action_set
                .process_grpc_response(self.current_index, msg);

            match result {
                Ok((next_msg, headers)) => {
                    let header_op =
                        headers.map(|hs| Operation::AddHeaders(HeadersOperation::new(hs)));
                    Ok((next_msg, header_op))
                }
                Err(grpc_err_resp) => Err(Operation::Die(grpc_err_resp)),
            }
        }

        pub fn runtime_action_set(&self) -> Rc<RuntimeActionSet> {
            Rc::clone(&self.runtime_action_set)
        }

        pub fn fail(self) -> Operation {
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
}

pub(crate) struct Filter {
    context_id: u32,
    index: Rc<ActionSetIndex>,
    header_resolver: Rc<HeaderResolver>,

    grpc_message_receiver_operation: Option<no_implicit_dep::GrpcMessageReceiverOperation>,
    headers_operations: Vec<HeadersOperation>,
}

impl Context for Filter {
    fn on_grpc_call_response(&mut self, _token_id: u32, status_code: u32, resp_size: usize) {
        let receiver = mem::take(&mut self.grpc_message_receiver_operation)
            .expect("We need an operation pending a gRPC response");
        let action_set = receiver.runtime_action_set();

        if status_code != Status::Ok as u32 {
            self.handle_operation(receiver.fail());
            return;
        }

        let response_body = match self.get_grpc_call_response_body(0, resp_size) {
            Some(body) => body,
            None => {
                self.handle_operation(receiver.fail());
                return;
            }
        };

        let result = receiver.digest_grpc_response(&response_body);
        match result {
            Ok((next_msg, header_op)) => {
                if let Some(header_op) = header_op {
                    self.handle_operation(header_op);
                }
                let receiver_op = self.handle_next_message(action_set, next_msg);
                self.handle_operation(receiver_op);
            }
            Err(die_op) => {
                self.handle_operation(die_op);
            }
        }
    }
}

impl HttpContext for Filter {
    fn on_http_request_headers(&mut self, _: usize, _: bool) -> Action {
        if let Some(action_sets) = self
            .index
            .get_longest_match_action_sets(self.request_authority().as_ref())
        {
            if let Some(action_set) = action_sets
                .iter()
                .find(|action_set| action_set.conditions_apply(/* self */))
            {
                let request_action = action_set.start_flow();
                let next_op = self.handle_next_message(Rc::clone(action_set), request_action);
                return self.handle_operation(next_op);
            }
        }
        Action::Continue
    }

    fn on_http_response_headers(&mut self, _num_headers: usize, _end_of_stream: bool) -> Action {
        let headers_operations = mem::take(&mut self.headers_operations);
        for op in headers_operations {
            for (header, value) in &op.headers() {
                self.add_http_response_header(header, value)
            }
        }
        Action::Continue
    }
}

impl Filter {
    fn handle_operation(&mut self, operation: Operation) -> Action {
        match operation {
            Operation::AwaitGrpcResponse(receiver_op) => {
                debug!("handle_operation: AwaitGrpcResponse");
                self.grpc_message_receiver_operation = Some(receiver_op);
                Action::Pause
            }
            Operation::AddHeaders(header_op) => {
                debug!("handle_operation: AddHeaders");
                self.headers_operations.push(header_op);
                Action::Continue
            }
            Operation::Die(die_op) => {
                debug!("handle_operation: Die");
                self.die(die_op);
                Action::Continue
            }
            Operation::Done() => {
                debug!("handle_operation: Done");
                self.resume_http_request();
                Action::Continue
            }
        }
    }

    fn handle_next_message(
        &mut self,
        action_set: Rc<RuntimeActionSet>,
        next_msg: Option<GrpcRequestAction>,
    ) -> Operation {
        match next_msg {
            Some(msg) => match self.send_grpc_request(&msg) {
                Ok(_token) => Operation::AwaitGrpcResponse(GrpcMessageReceiverOperation::new(
                    action_set,
                    msg.index(),
                )),
                Err(_status) => panic!("Error sending request"),
            },
            None => Operation::Done(),
        }
    }

    fn die(&mut self, die: GrpcErrResponse) {
        self.send_http_response(
            die.status_code(),
            die.headers(),
            Some(die.body().as_bytes()),
        );
    }

    fn request_authority(&self) -> String {
        match self.get_http_request_header(":authority") {
            None => {
                warn!(":authority header not found");
                String::new()
            }
            Some(host) => {
                let split_host = host.split(':').collect::<Vec<_>>();
                split_host[0].to_owned()
            }
        }
    }

    fn send_grpc_request(&self, req: &GrpcRequestAction) -> Result<u32, Status> {
        let headers = self
            .header_resolver
            .get_with_ctx(self)
            .iter()
            .map(|(header, value)| (*header, value.as_slice()))
            .collect();

        self.dispatch_grpc_call(
            req.upstream_name(),
            req.service_name(),
            req.method_name(),
            headers,
            req.message(),
            req.timeout(),
        )
    }

    pub fn new(
        context_id: u32,
        index: Rc<ActionSetIndex>,
        header_resolver: Rc<HeaderResolver>,
    ) -> Self {
        Self {
            context_id,
            index,
            header_resolver,
            grpc_message_receiver_operation: None,
            headers_operations: Vec::default(),
        }
    }
}
