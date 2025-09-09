use crate::action_set_index::ActionSetIndex;
use crate::configuration::FailureMode;
use crate::data::{AttributeOwner, AttributeResolver, PathCache};
use crate::filter::operations::{
    EventualOperation, ProcessGrpcMessageOperation, ProcessNextRequestOperation,
};
use crate::runtime_action::NextRequestResult;
use crate::runtime_action_set::RuntimeActionSet;
use crate::service::errors::{BuildMessageError, ProcessGrpcMessageError};
use crate::service::rate_limit::KUADRANT_REPORT_RATELIMIT_METHOD_NAME;
use crate::service::{DirectResponse, GrpcRequest, HeaderResolver, Headers, IndexedGrpcRequest};
use log::{debug, error};
use proxy_wasm::hostcalls;
use proxy_wasm::traits::{Context, HttpContext};
use proxy_wasm::types::{Action, BufferType, MapType, Status};
use std::rc::Rc;

type EventReceiver = (usize, Rc<RuntimeActionSet>);

enum Phase {
    RequestHeaders,
    RequestBody,
    ResponseHeaders,
    ResponseBody,
}

pub(crate) struct KuadrantFilter {
    context_id: u32,
    index: Rc<ActionSetIndex>,
    header_resolver: HeaderResolver,
    path_store: PathCache,
    grpc_message_receiver: Option<EventReceiver>,
    request_body_receiver: Option<(EventReceiver, String)>,
    response_body_receiver: Option<(EventReceiver, String)>,
    response_headers_to_add: Option<Headers>,
    request_headers_to_add: Option<Headers>,
    phase: Phase,
    response_content_type: Option<String>,
    sse_buffer: Option<String>,
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
                        }
                        FailureMode::Allow => {
                            // increment index to continue with next
                            if let Action::Continue = self.run(action_set, index + 1) {
                                self.done_processing_grpc_call_response();
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
                                Ok(ProcessGrpcMessageOperation::EventualOps(ops)) => {
                                    ops.into_iter().for_each(|op| {
                                        self.handle_eventual_operation(op);
                                    });

                                    // increment index to continue with next
                                    if let Action::Continue = self.run(action_set, index + 1) {
                                        self.done_processing_grpc_call_response();
                                    }
                                }
                                Ok(ProcessGrpcMessageOperation::DirectResponse(
                                    direct_response,
                                )) => {
                                    if let Phase::ResponseBody = self.phase {
                                        debug!("Ignoring trying to send direct response after phase has ended!");
                                        self.done_processing_grpc_call_response();
                                    } else {
                                        self.send_direct_response(direct_response);
                                    }
                                }
                                Err(ProcessGrpcMessageError::Protobuf(e)) => {
                                    // processing the response failed
                                    // The action failure mode is set to deny, so we log the error and die
                                    debug!("ProtobufError while processing grpc response: {e:?}");
                                    self.die();
                                }
                                Err(ProcessGrpcMessageError::Property(e)) => {
                                    // processing the response failed
                                    // The action failure mode is set to deny, so we log the error and die
                                    debug!("PropertyError while processing grpc response: {e:?}");
                                    self.die();
                                }
                                Err(e) => {
                                    // processing the response failed
                                    // The action failure mode is set to deny, so we log the error and die
                                    debug!(
                                        "Unexpected error while processing grpc response: {e:?}"
                                    );
                                    self.die();
                                }
                            }
                        }
                        None => {
                            match action_set.runtime_actions[index].get_failure_mode() {
                                FailureMode::Deny => {
                                    self.die();
                                }
                                FailureMode::Allow => {
                                    // increment index to continue with next
                                    if let Action::Continue = self.run(action_set, index + 1) {
                                        self.done_processing_grpc_call_response();
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
                return Action::Pause;
            }
        };

        if let Some(action_sets) = self.index.get_longest_match_action_sets(authority.as_ref()) {
            let action_set_opt = action_sets.iter().find_map(|action_set| {
                // returns the first non-None result,
                // namely when condition apply OR there is an error
                match action_set.conditions_apply(&mut self.path_store) {
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
                        // tracing headers need to be read at this request headers phase
                        self.header_resolver.get_with_ctx(self);

                        // unfortunately, lazy evaluation of request attributes is not possible here
                        // all request.* attributes need to be evaluated in advance
                        // and keep them in the path store
                        let req_attr_iter = action_set.request_attributes();
                        for attr in req_attr_iter.iter() {
                            if let Err(e) = self.path_store.resolve(attr) {
                                error!(
                                    "#{} on_http_request_headers: failed to read request attributes: {:?}",
                                    self.context_id, e
                                );
                                self.die();
                                return Action::Pause;
                            }
                        }

                        action = self.run(Rc::clone(action_set), 0);
                    }
                    Err(e) => {
                        error!(
                            "#{} on_http_request_headers: failed to apply conditions: {:?}",
                            self.context_id, e
                        );
                        self.die();
                        return Action::Pause;
                    }
                }
            }
        }

        action
    }

    fn on_http_request_body(&mut self, body_size: usize, end_of_stream: bool) -> Action {
        debug!(
            "#{} on_http_request_body: body_size: {body_size}, end_of_stream: {end_of_stream}",
            self.context_id
        );
        self.phase = Phase::RequestBody;
        // Need to check if there is something to do before expending
        // time and resources reading the body
        match self.request_body_receiver.take() {
            None => Action::Continue, // No pending actions, filter can continue normally
            Some(((index, action_set), transient_attr)) => {
                if !end_of_stream {
                    // This is not the end of the stream, so the complete request body is not yet available.
                    // Until JSON parsing is supported in streaming mode, the entire request body must be available.
                    // There is nothing to do here at the moment.
                    self.request_body_receiver = Some(((index, action_set), transient_attr));
                    return Action::Pause;
                }

                match self.get_http_request_body(0, body_size) {
                    Err(e) => {
                        error!(
                            "#{} on_http_request_body: failed to read the body: {:?}",
                            self.context_id, e
                        );
                        self.die();
                        Action::Pause
                    }
                    Ok(None) => {
                        error!(
                            "#{} on_http_request_body: expected some body bytes, but got None",
                            self.context_id
                        );
                        self.die();
                        Action::Pause
                    }
                    Ok(Some(body_bytes)) => match String::from_utf8(body_bytes) {
                        Err(e) => {
                            error!(
                                "#{} on_http_request_body: failed to convert body to string: {:?}",
                                self.context_id, e
                            );
                            self.die();
                            Action::Pause
                        }
                        Ok(body_str) => {
                            debug!(
                                "#{} on_http_request_body (size: {body_size}): action_set selected {}",
                                self.context_id, action_set.name
                            );
                            self.path_store
                                .add_transient(transient_attr.as_str(), body_str.into());
                            self.run(action_set, index)
                        }
                    },
                }
            }
        }
    }

    fn on_http_response_headers(&mut self, _num_headers: usize, _end_of_stream: bool) -> Action {
        debug!("#{} on_http_response_headers", self.context_id);
        self.phase = Phase::ResponseHeaders;

        // Detect Content-Type for SSE streaming support only if we are waiting for response body
        if self.response_body_receiver.is_some() {
            if let Ok(Some(content_type)) = self.get_http_response_header("content-type") {
                debug!("#{} Content-Type: {}", self.context_id, content_type);
                self.response_content_type = Some(content_type.clone());
            }
        }

        // response headers can only be added at this phase. At the response body time is already
        // too late
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

    fn on_http_response_body(&mut self, body_size: usize, end_of_stream: bool) -> Action {
        debug!(
            "#{} on_http_response_body: body_size: {body_size}, end_of_stream: {end_of_stream}",
            self.context_id
        );
        self.phase = Phase::ResponseBody;
        // Need to check if there is something to do before expending
        // time and resources reading the body
        match self.response_body_receiver.take() {
            None => Action::Continue, // No pending actions, filter can continue normally
            Some(((index, action_set), transient_attr)) => {
                // Check if this is an SSE stream and a TRLP action
                if let Some(ref content_type) = self.response_content_type {
                    if content_type.contains("text/event-stream")
                        && action_set.runtime_actions[index].grpc_service().method()
                            == KUADRANT_REPORT_RATELIMIT_METHOD_NAME
                    {
                        debug!(
                            "#{} Initializing SSE buffer for streaming response",
                            self.context_id
                        );
                        self.sse_buffer = Some(String::new());

                        return self.handle_sse_stream(
                            body_size,
                            end_of_stream,
                            index,
                            action_set,
                            transient_attr,
                        );
                    }
                }

                if !end_of_stream {
                    // This is not the end of the stream, so the complete request body is not yet available.
                    // Until JSON parsing is supported in streaming mode, the entire request body must be available.
                    // There is nothing to do here at the moment.
                    self.response_body_receiver = Some(((index, action_set), transient_attr));
                    return Action::Pause;
                }

                match self.get_http_response_body(0, body_size) {
                    Err(e) => {
                        error!(
                            "#{} get_http_response_body: failed to read the body: {:?}",
                            self.context_id, e
                        );
                        self.die();
                        Action::Continue
                    }
                    Ok(None) => {
                        error!(
                            "#{} get_http_response_body: expected some body bytes, but got None",
                            self.context_id
                        );
                        self.die();
                        Action::Continue
                    }
                    Ok(Some(body_bytes)) => match String::from_utf8(body_bytes) {
                        Err(e) => {
                            error!(
                                "#{} get_http_response_body: failed to convert body to string: {:?}",
                                self.context_id, e
                            );
                            self.die();
                            Action::Continue
                        }
                        Ok(body_str) => {
                            debug!(
                                "#{} on_http_response_body (size: {body_size}): action_set selected {}",
                                self.context_id, action_set.name
                            );
                            self.path_store
                                .add_transient(transient_attr.as_str(), body_str.into());
                            self.run(action_set, index)
                        }
                    },
                }
            }
        }
    }
}

impl KuadrantFilter {
    fn next_request(
        &mut self,
        action_set: &Rc<RuntimeActionSet>,
        start: usize,
    ) -> NextRequestResult {
        for (index, action) in action_set.runtime_actions.iter().skip(start).enumerate() {
            match action.build_request(&mut self.path_store) {
                Ok(None) => {
                    // This action does not build a request, continue to the next one
                    continue;
                }
                Ok(Some(grpc_request)) => {
                    return Ok(IndexedGrpcRequest::new(start + index, grpc_request).into());
                }
                Err(BuildMessageError::Evaluation(eval_err)) if eval_err.is_transient() => {
                    // this error indicates that some transient error happened
                    // This is dissmissed as an evaluation error and considered as "must wait" signal.
                    match eval_err.transient_property() {
                        Some(transient_attr) => {
                            match transient_attr {
                                "request_body" => {
                                    return Ok(ProcessNextRequestOperation::AwaitRequestBody(
                                        start + index,
                                        transient_attr.into(),
                                    ));
                                }
                                "response_body" => {
                                    return Ok(ProcessNextRequestOperation::AwaitResponseBody(
                                        start + index,
                                        transient_attr.into(),
                                    ));
                                }
                                _ => return Err(BuildMessageError::Evaluation(eval_err)), // transient
                                                                                          // property
                                                                                          // unknown
                            }
                        }
                        None => return Err(BuildMessageError::Evaluation(eval_err)), // transient
                                                                                     // property
                                                                                     // unknown
                    }
                }
                Err(e) => match action.get_failure_mode() {
                    FailureMode::Deny => return Err(e),
                    FailureMode::Allow => {
                        debug!("continuing as FailureMode Allow. error was {e:?}");
                        continue;
                    }
                },
            };
        }
        Ok(ProcessNextRequestOperation::Done)
    }

    fn run(&mut self, action_set: Rc<RuntimeActionSet>, start: usize) -> Action {
        let mut index = start;
        loop {
            match self.next_request(&action_set, index) {
                Ok(ProcessNextRequestOperation::Done) => {
                    // Nothing more to do, we can end the flow
                    return Action::Continue;
                }
                Ok(ProcessNextRequestOperation::GrpcRequest(indexed_req)) => {
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
                                    index += 1;
                                }
                            }
                        }
                    }
                }
                Ok(ProcessNextRequestOperation::AwaitRequestBody(indexed_req, transient_attr)) => {
                    // this arm indicates that the request body is not available
                    // must wait for the request body to be available
                    self.request_body_receiver = Some(((indexed_req, action_set), transient_attr));
                    return Action::Continue;
                }
                Ok(ProcessNextRequestOperation::AwaitResponseBody(indexed_req, transient_attr)) => {
                    // this arm indicates that the response body is not available
                    // must wait for the response body to be available
                    self.response_body_receiver = Some(((indexed_req, action_set), transient_attr));
                    return Action::Continue;
                }
                Err(err) => {
                    // Building the request failed
                    // The action failure mode is set to deny, so we log the error and die
                    debug!("Error while building request: {err:?}");
                    self.die();
                    return Action::Pause;
                }
            }
        }
    }

    fn handle_eventual_operation(&mut self, operation: EventualOperation) {
        match operation {
            EventualOperation::AddRequestHeaders(headers) => {
                if !headers.is_empty() {
                    if let Phase::RequestHeaders = self.phase {
                        if let Some(existing_headers) = self.request_headers_to_add.as_mut() {
                            existing_headers.extend(headers);
                        }
                    } else {
                        debug!("Ignoring trying to add request headers after phase has ended!");
                    }
                }
            }
            EventualOperation::AddResponseHeaders(headers) => {
                if !headers.is_empty() {
                    match self.phase {
                        Phase::RequestHeaders | Phase::RequestBody | Phase::ResponseHeaders => {
                            if let Some(existing_headers) = self.response_headers_to_add.as_mut() {
                                existing_headers.extend(headers);
                            }
                        }
                        _ => {
                            debug!(
                                "Ignoring trying to add response headers after phase has ended!"
                            );
                        }
                    }
                }
            }
        }
    }

    fn die(&self) {
        self.send_direct_response(DirectResponse::new_internal_server_error());
    }

    fn done_processing_grpc_call_response(&mut self) {
        match self.phase {
            Phase::RequestHeaders => {
                self.add_request_headers();
                let _ = self.resume_http_request();
            }
            Phase::RequestBody => {
                let _ = self.resume_http_request();
            }
            Phase::ResponseHeaders | Phase::ResponseBody => {
                let _ = self.resume_http_response();
            }
        }
    }

    fn send_direct_response(&self, direct_response: DirectResponse) {
        if let Phase::ResponseBody = self.phase {
            debug!("Ignoring trying to send direct response after phase has ended!");
        } else {
            let _ = self.send_http_response(
                direct_response.status_code(),
                direct_response.headers(),
                Some(direct_response.body().as_bytes()),
            );
        }
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
            path_store: PathCache::default(),
            grpc_message_receiver: None,
            request_body_receiver: None,
            response_body_receiver: None,
            response_headers_to_add: Some(Vec::default()),
            request_headers_to_add: Some(Vec::default()),
            phase: Phase::RequestHeaders,
            response_content_type: None,
            sse_buffer: None,
        }
    }

    fn handle_sse_stream(
        &mut self,
        body_size: usize,
        end_of_stream: bool,
        index: usize,
        action_set: Rc<RuntimeActionSet>,
        transient_attr: String,
    ) -> Action {
        debug!(
            "#{} handle_sse_stream: body_size: {body_size}, end_of_stream: {end_of_stream}",
            self.context_id
        );

        let chunk_bytes = match self.get_http_response_body(0, body_size) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                error!(
                    "#{} handle_sse_stream: no body bytes available",
                    self.context_id
                );
                self.die();
                return Action::Continue;
            }
            Err(e) => {
                error!(
                    "#{} handle_sse_stream: failed to read body: {:?}",
                    self.context_id, e
                );
                self.die();
                return Action::Continue;
            }
        };

        let chunk_str = match String::from_utf8(chunk_bytes) {
            Ok(s) => s,
            Err(e) => {
                error!(
                    "#{} handle_sse_stream: failed to convert bytes to string: {:?}",
                    self.context_id, e
                );
                self.die();
                return Action::Continue;
            }
        };

        debug!(
            "#{} handle_sse_stream: processing chunk: {}",
            self.context_id, chunk_str
        );

        if let Some(ref mut buffer) = self.sse_buffer {
            buffer.push_str(&chunk_str);

            loop {
                // Find the earliest frame delimiter ("\n\n" or "\r\n\r\n")
                let nn = buffer.find("\n\n");
                let rnrn = buffer.find("\r\n\r\n");
                let delim_index = match (nn, rnrn) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (Some(a), None) => Some(a),
                    (None, Some(b)) => Some(b),
                    (None, None) => None,
                };

                let Some(end_idx) = delim_index else { break };

                let frame = buffer[..end_idx].to_string();

                let delim_len = if buffer[end_idx..].starts_with("\r\n\r\n") {
                    4
                } else {
                    2
                };
                buffer.drain(..end_idx + delim_len);

                let mut data_buf = String::new();
                for line in frame.lines() {
                    if let Some(rest) = line.strip_prefix("data:") {
                        let value = if let Some(stripped) = rest.strip_prefix(' ') {
                            stripped
                        } else {
                            rest
                        };
                        if !data_buf.is_empty() {
                            data_buf.push('\n');
                        }
                        data_buf.push_str(value);
                    }
                }

                if data_buf.is_empty() {
                    continue;
                }

                debug!(
                    "#{} handle_sse_stream: processing event data: {}",
                    self.context_id, data_buf
                );

                if let Some(usage_json) = extract_usage_from_data(&data_buf) {
                    debug!(
                        "#{} handle_sse_stream: found usage data: {}",
                        self.context_id, usage_json
                    );

                    self.path_store
                        .add_transient(transient_attr.as_str(), usage_json.into());

                    return self.run(action_set, index);
                }
            }
        }

        if !end_of_stream {
            self.response_body_receiver = Some(((index, action_set), transient_attr));
            return Action::Continue;
        }

        debug!(
            "#{} handle_sse_stream: stream ended but no usage data found",
            self.context_id
        );

        Action::Continue
    }
}

fn extract_usage_from_data(data: &str) -> Option<String> {
    if data.trim() == "[DONE]" {
        return None;
    }
    match serde_json::from_str::<serde_json::Value>(data) {
        Ok(json) => {
            if let Some(usage) = json.get("usage") {
                if usage.is_null() {
                    return None;
                }
                let usage_json = serde_json::json!({"usage": usage});
                Some(usage_json.to_string())
            } else {
                None
            }
        }
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::extract_usage_from_data;

    #[test]
    fn extract_usage_from_data_with_usage() {
        let data =
            r#"{"id":"x","usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3}}"#;
        let result = extract_usage_from_data(data);
        assert_eq!(
            result,
            Some(
                "{\"usage\":{\"completion_tokens\":2,\"prompt_tokens\":1,\"total_tokens\":3}}"
                    .to_string()
            )
        );
    }

    #[test]
    fn extract_usage_from_data_without_usage() {
        let data = r#"{"id":"x","choices":[]}"#;
        let result = extract_usage_from_data(data);
        assert!(result.is_none());
    }

    #[test]
    fn extract_usage_from_data_invalid_json() {
        let data = "not-json";
        let result = extract_usage_from_data(data);
        assert!(result.is_none());
    }

    #[test]
    fn extract_usage_from_data_done_event() {
        let data = "[DONE]";
        let result = extract_usage_from_data(data);
        assert!(result.is_none());
    }

    #[test]
    fn extract_usage_from_data_with_null_usage() {
        let data = r#"{"id":"x","usage":null}"#;
        let result = extract_usage_from_data(data);
        assert!(result.is_none());
    }
}
