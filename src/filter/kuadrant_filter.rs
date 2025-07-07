use crate::action_set_index::ActionSetIndex;
use crate::configuration::FailureMode;
use crate::filter::operations::{EventualOperation, Operation};
use crate::runtime_action::IndexedRequestResult;
use crate::runtime_action_set::RuntimeActionSet;
use crate::service::errors::ProcessGrpcMessageError;
use crate::service::{DirectResponse, GrpcRequest, HeaderResolver, Headers, IndexedGrpcRequest};
use log::{debug, error, warn};
use proxy_wasm::hostcalls;
use proxy_wasm::traits::{Context, HttpContext};
use proxy_wasm::types::{Action, BufferType, MapType, Status};
use std::rc::Rc;

type GrpcMessageReceiver = (usize, Rc<RuntimeActionSet>);

pub(crate) struct KuadrantFilter {
    context_id: u32,
    index: Rc<ActionSetIndex>,
    header_resolver: HeaderResolver,

    grpc_message_receiver: Option<GrpcMessageReceiver>,
    response_headers_to_add: Option<Headers>,
    request_headers_to_add: Option<Headers>,
}

impl Context for KuadrantFilter {
    fn on_grpc_call_response(&mut self, token_id: u32, status_code: u32, resp_size: usize) {
        debug!(
            "#{} on_grpc_call_response: received gRPC call response: token: {token_id}, status: {status_code}",
            self.context_id
        );

        match self.grpc_message_receiver.take() {
            Some((index, action_set)) => {
                if status_code != Status::Ok as u32 {
                    match action_set.runtime_actions[index].get_failure_mode() {
                        FailureMode::Deny => {
                            self.die();
                            return;
                        }
                        FailureMode::Allow => {
                            // increment index to continue with next
                            if let Action::Continue = self.run(action_set, index + 1) {
                                self.done();
                                return;
                            }
                        }
                    }
                } else {
                    match hostcalls::get_buffer(BufferType::GrpcReceiveBuffer, 0, resp_size)
                        .unwrap_or_else(|e| {
                            // get_buffer panics instead of returning an Error so this will not happen
                            error!(
                                "on_grpc_call_response failed to read gRPC receive buffer: `{:?}`",
                                e
                            );
                            None
                        }) {
                        Some(response_body) => {
                            match action_set.runtime_actions[index].process_response(&response_body)
                            {
                                Ok(Operation::EventualOps(ops)) => {
                                    ops.into_iter().for_each(|op| {
                                        self.handle_eventual_operation(op);
                                    });

                                    // increment index to continue with next
                                    if let Action::Continue = self.run(action_set, index + 1) {
                                        self.done();
                                        return;
                                    }
                                }
                                Ok(Operation::DirectResponse(direct_response)) => {
                                    self.send_http_reponse(direct_response);
                                    return;
                                }
                                Err(ProcessGrpcMessageError::Protobuf(e)) => {
                                    // processing the response failed
                                    // The action failure mode is set to deny, so we log the error and die
                                    debug!("ProtobufError while processing grpc response: {e:?}");
                                    self.die();
                                    return;
                                }
                                Err(ProcessGrpcMessageError::Property(e)) => {
                                    // processing the response failed
                                    // The action failure mode is set to deny, so we log the error and die
                                    debug!("PropertyError while processing grpc response: {e:?}");
                                    self.die();
                                    return;
                                }
                                Err(e) => {
                                    // processing the response failed
                                    // The action failure mode is set to deny, so we log the error and die
                                    debug!(
                                        "Unexpected error while processing grpc response: {e:?}"
                                    );
                                    self.die();
                                    return;
                                }
                            }
                        }
                        None => {
                            match action_set.runtime_actions[index].get_failure_mode() {
                                FailureMode::Deny => {
                                    self.die();
                                    return;
                                }
                                FailureMode::Allow => {
                                    // increment index to continue with next
                                    if let Action::Continue = self.run(action_set, index + 1) {
                                        self.done();
                                        return;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            None => {
                error!(
                    "#{} on_grpc_call_response: received gRPC response but no pending receiver",
                    self.context_id
                );
                self.die()
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
                self.die();
                return Action::Continue;
            }
        };

        if let Some(action_sets) = self.index.get_longest_match_action_sets(authority.as_ref()) {
            let action_set_opt = action_sets.iter().find_map(|action_set| {
                // returns the first non-None result,
                // namely when condition apply OR there is an error
                match action_set.conditions_apply() {
                    Ok(true) => Some(Ok(action_set)),
                    Ok(false) => None,
                    Err(e) => Some(Err(e)),
                }
            });

            if let Some(action_set_res) = action_set_opt {
                match action_set_res {
                    Ok(action_set) => {
                        debug!(
                            "#{} action_set selected {}",
                            self.context_id, action_set.name
                        );
                        action = self.run(Rc::clone(action_set), 0);
                    }
                    Err(e) => {
                        error!(
                            "#{} on_http_request_headers: failed to apply conditions: {:?}",
                            self.context_id, e
                        );
                        self.die();
                        return Action::Continue;
                    }
                }
            }
        }

        action
    }

    fn on_http_response_headers(&mut self, _num_headers: usize, _end_of_stream: bool) -> Action {
        debug!("#{} on_http_response_headers", self.context_id);
        if let Some(response_headers) = self.response_headers_to_add.take() {
            for (header, value) in response_headers {
                if let Err(status) = self.add_http_response_header(header.as_str(), value.as_str())
                {
                    log::error!(
                        "#{} on_http_response_headers: failed to add headers: {:?}",
                        self.context_id,
                        status
                    );
                }
            }
        }
        Action::Continue
    }
}

impl KuadrantFilter {
    fn next_request(
        &mut self,
        action_set: &Rc<RuntimeActionSet>,
        start: usize,
    ) -> IndexedRequestResult {
        for (index, action) in action_set.runtime_actions.iter().skip(start).enumerate() {
            match action.build_request()? {
                None => continue,
                Some(grpc_request) => {
                    return Ok(Some(IndexedGrpcRequest::new(start + index, grpc_request)));
                }
            }
        }
        Ok(None)
    }

    fn run(&mut self, action_set: Rc<RuntimeActionSet>, start: usize) -> Action {
        let mut index = start;
        loop {
            match self.next_request(&action_set, index) {
                Ok(None) => {
                    // Nothing more to do, we can end the flow
                    return Action::Continue;
                }
                Ok(Some(indexed_req)) => {
                    index = indexed_req.index();
                    match self.send_grpc_request(indexed_req.request()) {
                        Ok(_token) => {
                            self.grpc_message_receiver = Some((index, action_set));
                            return Action::Pause;
                        }
                        Err(status) => {
                            debug!(
                                "#{} run: failed to send grpc request `{status:?}`",
                                self.context_id
                            );
                            // if failure mode is set to allow, continue with next action
                            match action_set.runtime_actions[index].get_failure_mode() {
                                FailureMode::Deny => {
                                    self.die();
                                    return Action::Pause;
                                }
                                FailureMode::Allow => {
                                    // increment index to continue with next
                                    index = index + 1;
                                }
                            }
                        }
                    }
                }
                Err(err) => {
                    // Building the request failed
                    // The action failure mode is set to deny, so we log the error and die
                    debug!("Error while building request: {:?}", err);
                    self.die();
                    return Action::Pause;
                }
            }
        }
    }

    fn handle_eventual_operation(&mut self, operation: EventualOperation) {
        match operation {
            EventualOperation::AddRequestHeaders(headers) => {
                if let Some(existing_headers) = self.request_headers_to_add.as_mut() {
                    existing_headers.extend(headers);
                } else {
                    warn!("Trying to add request headers after phase has ended!")
                }
            }
            EventualOperation::AddResponseHeaders(headers) => {
                if let Some(existing_headers) = self.response_headers_to_add.as_mut() {
                    existing_headers.extend(headers);
                } else {
                    warn!("Trying to add response headers after phase has ended!")
                }
            }
        }
    }

    fn die(&self) {
        self.send_http_reponse(DirectResponse::new_internal_server_error());
    }

    fn done(&mut self) {
        self.add_request_headers();
        let _ = self.resume_http_request();
    }

    fn send_http_reponse(&self, direct_response: DirectResponse) {
        let _ = self.send_http_response(
            direct_response.status_code(),
            direct_response.headers(),
            Some(direct_response.body().as_bytes()),
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
                // get_map_value panics instead of returning an Error so this will not happen
                error!("failed to retrieve :authority header: {:?}", e);
                Err(e)
            }
        }
    }

    fn send_grpc_request(&self, req: GrpcRequest) -> Result<u32, Status> {
        debug!(
            "#{} send_grpc_request: {} {} {} {:?}",
            self.context_id,
            req.upstream_name(),
            req.service_name(),
            req.method_name(),
            req.timeout(),
        );
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
        if let Some(request_headers) = self.request_headers_to_add.take() {
            for (header, value) in request_headers {
                if let Err(status) = self.add_http_request_header(header.as_str(), value.as_str()) {
                    log::error!(
                        "add_http_request_headers failed for {}: {:?}",
                        &header,
                        status
                    );
                }
            }
        }
    }

    pub fn new(
        context_id: u32,
        index: Rc<ActionSetIndex>,
        header_resolver: HeaderResolver,
    ) -> Self {
        Self {
            context_id,
            index,
            header_resolver,
            grpc_message_receiver: None,
            response_headers_to_add: Some(Vec::default()),
            request_headers_to_add: Some(Vec::default()),
        }
    }
}
