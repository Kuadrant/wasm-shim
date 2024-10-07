pub(crate) mod auth;
pub(crate) mod grpc_message;
pub(crate) mod rate_limit;

use crate::configuration::{Action, Extension, ExtensionType, FailureMode};
use crate::operation_dispatcher::Operation;
use crate::service::auth::{AuthService, AUTH_METHOD_NAME, AUTH_SERVICE_NAME};
use crate::service::grpc_message::{GrpcMessageRequest, GrpcMessageResponse};
use crate::service::rate_limit::{RateLimitService, RATELIMIT_METHOD_NAME, RATELIMIT_SERVICE_NAME};
use crate::service::TracingHeader::{Baggage, Traceparent, Tracestate};
use log::warn;
use protobuf::Message;
use proxy_wasm::hostcalls;
use proxy_wasm::types::{BufferType, Bytes, MapType, Status};
use std::cell::OnceCell;
use std::rc::Rc;
use std::time::Duration;

#[derive(Default)]
pub struct GrpcService {
    #[allow(dead_code)]
    extension: Rc<Extension>,
    name: &'static str,
    method: &'static str,
}

impl GrpcService {
    pub fn new(extension: Rc<Extension>) -> Self {
        match extension.extension_type {
            ExtensionType::Auth => Self {
                extension,
                name: AUTH_SERVICE_NAME,
                method: AUTH_METHOD_NAME,
            },
            ExtensionType::RateLimit => Self {
                extension,
                name: RATELIMIT_SERVICE_NAME,
                method: RATELIMIT_METHOD_NAME,
            },
        }
    }

    fn endpoint(&self) -> &str {
        &self.extension.endpoint
    }
    fn name(&self) -> &str {
        self.name
    }
    fn method(&self) -> &str {
        self.method
    }
    #[allow(dead_code)]
    pub fn failure_mode(&self) -> &FailureMode {
        &self.extension.failure_mode
    }

    pub fn process_grpc_response(operation: Rc<Operation>, resp_size: usize) {
        let failure_mode = operation.get_failure_mode();
        if let Some(res_body_bytes) = hostcalls::get_buffer(BufferType::GrpcReceiveBuffer, 0, resp_size).unwrap() {
            match GrpcMessageResponse::new(operation.get_extension_type(), &res_body_bytes) {
                Ok(res)  =>  {
                    match operation.get_extension_type() {
                        ExtensionType::Auth => AuthService::process_auth_grpc_response(res, failure_mode),
                        ExtensionType::RateLimit => {
                            RateLimitService::process_ratelimit_grpc_response(res, failure_mode)
                        }
                    }
                }
                Err(e) => {
                    warn!("failed to parse grpc response body into GrpcMessageResponse message: {e}");
                    GrpcService::handle_error_on_grpc_response(failure_mode);
                }
            };
        }
        else {
                warn!("grpc response body is empty!");
                GrpcService::handle_error_on_grpc_response(failure_mode);
        }
    }

    pub fn handle_error_on_grpc_response(failure_mode: &FailureMode) {
        match failure_mode {
            FailureMode::Deny => {
                hostcalls::send_http_response(500, vec![], Some(b"Internal Server Error.\n"))
                    .unwrap()
            }
            FailureMode::Allow => hostcalls::resume_http_request().unwrap(),
        }
    }
}

pub type GrpcCallFn = fn(
    upstream_name: &str,
    service_name: &str,
    method_name: &str,
    initial_metadata: Vec<(&str, &[u8])>,
    message: Option<&[u8]>,
    timeout: Duration,
) -> Result<u32, Status>;

pub type GetMapValuesBytesFn = fn(map_type: MapType, key: &str) -> Result<Option<Bytes>, Status>;

pub type GrpcMessageBuildFn =
    fn(extension_type: &ExtensionType, action: &Action) -> Option<GrpcMessageRequest>;

pub struct GrpcServiceHandler {
    service: Rc<GrpcService>,
    header_resolver: Rc<HeaderResolver>,
}

impl GrpcServiceHandler {
    pub fn new(service: Rc<GrpcService>, header_resolver: Rc<HeaderResolver>) -> Self {
        Self {
            service,
            header_resolver,
        }
    }

    pub fn send(
        &self,
        get_map_values_bytes_fn: GetMapValuesBytesFn,
        grpc_call_fn: GrpcCallFn,
        message: GrpcMessageRequest,
    ) -> Result<u32, Status> {
        let msg = Message::write_to_bytes(&message).unwrap();
        let metadata = self
            .header_resolver
            .get(get_map_values_bytes_fn)
            .iter()
            .map(|(header, value)| (*header, value.as_slice()))
            .collect();

        grpc_call_fn(
            self.service.endpoint(),
            self.service.name(),
            self.service.method(),
            metadata,
            Some(&msg),
            Duration::from_secs(5),
        )
    }

    pub fn get_extension(&self) -> Rc<Extension> {
        Rc::clone(&self.service.extension)
    }
}

pub struct HeaderResolver {
    headers: OnceCell<Vec<(&'static str, Bytes)>>,
}

impl Default for HeaderResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl HeaderResolver {
    pub fn new() -> Self {
        Self {
            headers: OnceCell::new(),
        }
    }

    pub fn get(&self, get_map_values_bytes_fn: GetMapValuesBytesFn) -> &Vec<(&'static str, Bytes)> {
        self.headers.get_or_init(|| {
            let mut headers = Vec::new();
            for header in TracingHeader::all() {
                if let Ok(Some(value)) =
                    get_map_values_bytes_fn(MapType::HttpRequestHeaders, (*header).as_str())
                {
                    headers.push(((*header).as_str(), value));
                }
            }
            headers
        })
    }
}

// tracing headers
pub enum TracingHeader {
    Traceparent,
    Tracestate,
    Baggage,
}

impl TracingHeader {
    fn all() -> &'static [Self; 3] {
        &[Traceparent, Tracestate, Baggage]
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Traceparent => "traceparent",
            Tracestate => "tracestate",
            Baggage => "baggage",
        }
    }
}
