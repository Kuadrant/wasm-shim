use crate::action_set_index::ActionSetIndex;
use crate::filter::proposal_context::no_implicit_dep::{
    EndRequestOperation, GrpcMessageSenderOperation, HeadersOperation, Operation,
};
use crate::service::{GrpcErrResponse, GrpcRequest, HeaderResolver};
use log::{debug, error, warn};
use proxy_wasm::traits::{Context, HttpContext};
use proxy_wasm::types::{Action, Status};
use std::mem;
use std::rc::Rc;

pub mod no_implicit_dep {
    use crate::runtime_action_set::RuntimeActionSet;
    use crate::service::{GrpcErrResponse, GrpcRequest};
    use log::error;
    use std::cell::OnceCell;
    use std::rc::Rc;

    #[allow(dead_code)]
    pub enum Operation {
        SendGrpcRequest(GrpcMessageSenderOperation),
        AwaitGrpcResponse(GrpcMessageReceiverOperation),
        AddHeaders(HeadersOperation),
        Die(GrpcErrResponse),
        //todo(adam-cattermole): does Done make sense? in this case no PendingOperation
        // instead just Option<PendingOperation>?
        Done(),
    }

    pub struct GrpcMessageSenderOperation {
        runtime_action_set: Rc<RuntimeActionSet>,
        current_index: usize,
    }

    impl GrpcMessageSenderOperation {
        pub fn new(runtime_action_set: Rc<RuntimeActionSet>, current_index: usize) -> Self {
            Self {
                runtime_action_set,
                current_index,
            }
        }

        //todo(adam-cattermole): should this return a tuple? alternative?
        pub fn next_grpc_request(self) -> (Option<GrpcRequest>, Operation) {
            let (index, msg) = self
                .runtime_action_set
                .find_next_grpc_request(self.current_index);
            match msg {
                None => (None, Operation::Done()),
                Some(_) => (
                    msg,
                    Operation::AwaitGrpcResponse(GrpcMessageReceiverOperation {
                        runtime_action_set: self.runtime_action_set,
                        current_index: index,
                    }),
                ),
            }
        }
    }

    pub struct GrpcMessageReceiverOperation {
        runtime_action_set: Rc<RuntimeActionSet>,
        current_index: usize,
    }

    impl GrpcMessageReceiverOperation {
        pub fn digest_grpc_response(self, msg: &[u8]) -> Operation {
            let action = self
                .runtime_action_set
                .runtime_actions
                .get(self.current_index)
                .unwrap();

            let next_op = action.process_response(msg);
            match next_op {
                Operation::AddHeaders(mut op) => {
                    op.set_action_set_index(self.runtime_action_set, self.current_index);
                    Operation::AddHeaders(op)
                }
                Operation::Done() => Operation::SendGrpcRequest(GrpcMessageSenderOperation::new(
                    self.runtime_action_set,
                    self.current_index + 1,
                )),
                _ => next_op,
            }
        }

        pub fn fail(self) -> Operation {
            Operation::Die(GrpcErrResponse::new_internal_server_error())
        }
    }

    pub struct HeadersOperation {
        headers: Vec<(String, String)>,
        runtime_action_set: OnceCell<Rc<RuntimeActionSet>>,
        current_index: usize,
    }

    impl HeadersOperation {
        pub fn new(headers: Vec<(String, String)>) -> Self {
            Self {
                headers,
                runtime_action_set: OnceCell::new(),
                current_index: 0,
            }
        }

        pub fn set_action_set_index(
            &mut self,
            action_set_index: Rc<RuntimeActionSet>,
            index: usize,
        ) {
            match self.runtime_action_set.set(action_set_index) {
                Ok(_) => self.current_index = index,
                Err(_) => error!("Error setting action set index, already set"),
            }
        }

        pub fn progress(&self) -> Operation {
            let next_op = match self.runtime_action_set.get() {
                None => panic!("Invalid state, called progress without setting runtime action set"),
                Some(runtime_action_set) => {
                    Operation::SendGrpcRequest(GrpcMessageSenderOperation::new(
                        Rc::clone(runtime_action_set),
                        self.current_index + 1,
                    ))
                }
            };
            next_op
        }

        pub fn headers(self) -> Vec<(String, String)> {
            self.headers
        }
    }

    pub struct EndRequestOperation {
        pub status: u32,
        pub headers: Vec<(String, String)>,
        pub body: Option<String>,
    }

    impl EndRequestOperation {
        pub fn new(status: u32, headers: Vec<(String, String)>, body: Option<String>) -> Self {
            Self {
                status,
                headers,
                body,
            }
        }

        pub fn new_with_status(status: u32) -> Self {
            Self::new(status, Vec::default(), None)
        }

        // todo(adam-cattermole): perhaps we should be more explicit with a different function?
        // Default Die is with 500 Internal Server Error.
        pub fn default() -> Self {
            Self::new(
                500,
                Vec::default(),
                Some("Internal Server Error.\n".to_string()),
            )
        }

        pub fn headers(&self) -> Vec<(&str, &str)> {
            self.headers
                .iter()
                .map(|(header, value)| (header.as_str(), value.as_str()))
                .collect()
        }

        pub fn body(&self) -> Option<&[u8]> {
            self.body.as_deref().map(|s| s.as_bytes())
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
        let next = if status_code == Status::Ok as u32 {
            match self.get_grpc_call_response_body(0, resp_size) {
                Some(response_body) => receiver.digest_grpc_response(&response_body),
                None => receiver.fail(),
            }
        } else {
            receiver.fail()
        };
        self.handle_operation(next);
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
                let op = Operation::SendGrpcRequest(GrpcMessageSenderOperation::new(
                    Rc::clone(action_set),
                    0,
                ));
                return self.handle_operation(op);
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
            Operation::SendGrpcRequest(sender_op) => {
                debug!("handle_operation: SendGrpcRequest");
                let (msg, op) = sender_op.next_grpc_request();
                match msg {
                    None => self.handle_operation(op),
                    Some(m) => match self.send_grpc_request(m) {
                        Ok(_token) => self.handle_operation(op),
                        Err(_status) => panic!("Error sending request"),
                    },
                }
            }
            Operation::AwaitGrpcResponse(receiver_op) => {
                debug!("handle_operation: AwaitGrpcResponse");
                self.grpc_message_receiver_operation = Some(receiver_op);
                Action::Pause
            }
            Operation::AddHeaders(header_op) => {
                debug!("handle_operation: AddHeaders");
                let next = header_op.progress();
                self.headers_operations.push(header_op);
                self.handle_operation(next)
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

    fn send_grpc_request(&self, req: GrpcRequest) -> Result<u32, Status> {
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
