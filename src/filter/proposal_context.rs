use crate::action_set_index::ActionSetIndex;
use crate::filter::proposal_context::no_implicit_dep::{EndRequestOperation, HeadersOperation};
use log::warn;
use proxy_wasm::traits::{Context, HttpContext};
use proxy_wasm::types::{Action, Status};
use std::mem;

pub mod no_implicit_dep {
    use proxy_wasm::traits::HttpContext;

    #[allow(dead_code)]
    pub enum Operation {
        AwaitGrpcResponse(GrpcMessageReceiverOperation),
        AddHeaders(HeadersOperation),
        Die(EndRequestOperation),
    }
    pub struct GrpcMessageReceiverOperation {}

    impl GrpcMessageReceiverOperation {
        pub fn process<T: HttpContext>(self, _msg: &[u8], _ctx: &mut T) -> Operation {
            todo!()
        }

        pub fn fail<T: HttpContext>(self, _ctx: &mut T) -> Operation {
            Operation::Die(EndRequestOperation { status: 500 })
        }
    }

    pub struct HeadersOperation {}

    pub struct EndRequestOperation {
        pub status: u32,
    }
}

struct Filter {
    index: ActionSetIndex,

    grpc_message_receiver_operation: Option<no_implicit_dep::GrpcMessageReceiverOperation>,
    headers_operations: Vec<HeadersOperation>,
}

impl Context for Filter {
    fn on_grpc_call_response(&mut self, _token_id: u32, status_code: u32, _resp_size: usize) {
        let receiver = mem::take(&mut self.grpc_message_receiver_operation)
            .expect("We need an operation pending a gRPC response");
        let next = if status_code != Status::Ok as u32 {
            receiver.process(&[], self)
        } else {
            receiver.fail(self)
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
                self.handle_operation(action_set.start_flow(self))
            }
        }
        Action::Continue
    }

    fn on_http_response_headers(&mut self, _num_headers: usize, _end_of_stream: bool) -> Action {
        for _op in self.headers_operations.drain(..) {
            todo!("Add the headers")
        }
        Action::Continue
    }
}

impl Filter {
    fn handle_operation(&mut self, operation: no_implicit_dep::Operation) {
        match operation {
            no_implicit_dep::Operation::AwaitGrpcResponse(msg) => {
                self.grpc_message_receiver_operation = Some(msg)
            }
            no_implicit_dep::Operation::AddHeaders(headers) => {
                self.headers_operations.push(headers)
            }
            no_implicit_dep::Operation::Die(die) => self.die(die),
        }
    }

    fn die(&mut self, die: EndRequestOperation) {
        self.send_http_response(die.status, Vec::default(), None);
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
}
