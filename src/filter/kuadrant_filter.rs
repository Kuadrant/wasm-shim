use crate::action_set_index::ActionSetIndex;
use crate::filter::operations::{
    GrpcMessageReceiverOperation, GrpcMessageSenderOperation, Operation,
};
use crate::runtime_action_set::RuntimeActionSet;
use crate::service::{GrpcErrResponse, GrpcRequest, HeaderResolver, Headers};
use log::{debug, error, warn};
use proxy_wasm::hostcalls;
use proxy_wasm::traits::{Context, HttpContext};
use proxy_wasm::types::{Action, BufferType, MapType, Status};
use std::mem;
use std::rc::Rc;

pub(crate) struct KuadrantFilter {
    context_id: u32,
    index: Rc<ActionSetIndex>,
    header_resolver: Rc<HeaderResolver>,

    grpc_message_receiver_operation: Option<GrpcMessageReceiverOperation>,
    response_headers_to_add: Option<Headers>,
    request_headers_to_add: Option<Headers>,
}

impl Context for KuadrantFilter {
    fn on_grpc_call_response(&mut self, token_id: u32, status_code: u32, resp_size: usize) {
        debug!(
            "#{} on_grpc_call_response: received gRPC call response: token: {token_id}, status: {status_code}",
            self.context_id
        );

        match mem::take(&mut self.grpc_message_receiver_operation) {
            Some(receiver) => {
                let mut ops = Vec::new();

                if status_code != Status::Ok as u32 {
                    ops.push(receiver.fail());
                } else if let Some(response_body) =
                    hostcalls::get_buffer(BufferType::GrpcReceiveBuffer, 0, resp_size)
                        .unwrap_or_else(|e| {
                            error!(
                                "on_grpc_call_response failed to read gRPC receive buffer: `{:?}`",
                                e
                            );
                            None
                        })
                {
                    ops.extend(receiver.digest_grpc_response(&response_body));
                } else {
                    ops.push(receiver.fail());
                }

                ops.into_iter().for_each(|op| {
                    self.handle_operation(op);
                })
            }
            None => {
                error!(
                    "#{} on_grpc_call_response: received gRPC response but no pending receiver",
                    self.context_id
                );
                self.die(GrpcErrResponse::new_internal_server_error())
            }
        }
    }
}

impl HttpContext for KuadrantFilter {
    fn on_http_request_headers(&mut self, _: usize, _: bool) -> Action {
        debug!("#{} on_http_request_headers", self.context_id);

        #[cfg(feature = "debug-host-behaviour")]
        crate::data::debug_all_well_known_attributes();

        // default action if we find no action_set where conditions apply
        let mut action = Action::Continue;

        let authority = match self.request_authority() {
            Ok(authority) => authority,
            Err(_) => {
                self.die(GrpcErrResponse::new_internal_server_error());
                return Action::Continue;
            }
        };

        if let Some(action_sets) = self.index.get_longest_match_action_sets(authority.as_ref()) {
            for action_set in action_sets {
                match action_set.conditions_apply() {
                    Ok(true) => {
                        debug!(
                            "#{} action_set selected {}",
                            self.context_id, action_set.name
                        );
                        action = self.start_flow(Rc::clone(action_set));
                        break;
                    }
                    Ok(false) => continue,
                    Err(e) => {
                        error!(
                            "#{} on_http_request_headers: failed to apply conditions: {:?}",
                            self.context_id, e
                        );
                        self.die(GrpcErrResponse::new_internal_server_error());
                        return Action::Continue;
                    }
                }
            }
        }

        if action == Action::Continue {
            // the request headers are currently always None, however this is one of two phases
            // where headers should be added
            self.add_request_headers()
        }
        action
    }

    fn on_http_response_headers(&mut self, _num_headers: usize, _end_of_stream: bool) -> Action {
        debug!("#{} on_http_response_headers", self.context_id);
        if let Some(response_headers) = mem::take(&mut self.response_headers_to_add) {
            for (header, value) in response_headers {
                self.add_http_response_header(header.as_str(), value.as_str())
            }
        }
        Action::Continue
    }
}

impl KuadrantFilter {
    fn start_flow(&mut self, action_set: Rc<RuntimeActionSet>) -> Action {
        let grpc_request = action_set.find_first_grpc_request();
        let op = match grpc_request {
            Ok(None) => Operation::Done(),
            Ok(Some(indexed_req)) => {
                Operation::SendGrpcRequest(GrpcMessageSenderOperation::new(action_set, indexed_req))
            }
            Err(grpc_err_response) => Operation::Die(grpc_err_response),
        };
        self.handle_operation(op)
    }

    fn handle_operation(&mut self, operation: Operation) -> Action {
        match operation {
            Operation::SendGrpcRequest(sender_op) => {
                debug!("handle_operation: SendGrpcRequest");
                let next_op = {
                    let (req, receiver_op) = sender_op.build_receiver_operation();
                    match self.send_grpc_request(req) {
                        Ok(_token) => Operation::AwaitGrpcResponse(receiver_op),
                        Err(status) => {
                            debug!("handle_operation: failed to send grpc request `{status:?}`");
                            receiver_op.fail()
                        }
                    }
                };
                self.handle_operation(next_op)
            }
            Operation::AwaitGrpcResponse(receiver_op) => {
                debug!("handle_operation: AwaitGrpcResponse");
                self.grpc_message_receiver_operation = Some(receiver_op);
                Action::Pause
            }
            Operation::AddHeaders(header_op) => {
                debug!("handle_operation: AddHeaders");
                match header_op.into_inner() {
                    crate::service::HeaderKind::Request(headers) => {
                        if let Some(existing_headers) = self.request_headers_to_add.as_mut() {
                            existing_headers.extend(headers);
                        } else {
                            warn!("Trying to add request headers after phase has ended!")
                        }
                    }
                    crate::service::HeaderKind::Response(headers) => {
                        if let Some(existing_headers) = self.response_headers_to_add.as_mut() {
                            existing_headers.extend(headers);
                        } else {
                            warn!("Trying to add response headers after phase has ended!")
                        }
                    }
                }
                Action::Continue
            }
            Operation::Die(die_op) => {
                debug!("handle_operation: Die");
                self.die(die_op);
                Action::Continue
            }
            Operation::Done() => {
                debug!("handle_operation: Done");
                self.add_request_headers();
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

    fn request_authority(&self) -> Result<String, Status> {
        match hostcalls::get_map_value(MapType::HttpRequestHeaders, ":authority") {
            Ok(Some(host)) => {
                let split_host = host.split_once(':').map_or(host.as_str(), |(h, _)| h);
                Ok(split_host.to_owned())
            }
            Ok(None) => {
                error!(":authority header not found");
                Err(Status::NotFound)
            }
            Err(e) => {
                error!("failed to retrieve :authority header: {:?}", e);
                Err(e)
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

    fn add_request_headers(&mut self) {
        if let Some(request_headers) = mem::take(&mut self.request_headers_to_add) {
            for (header, value) in request_headers {
                self.add_http_request_header(header.as_str(), value.as_str())
            }
        }
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
            response_headers_to_add: Some(Vec::default()),
            request_headers_to_add: Some(Vec::default()),
        }
    }
}
