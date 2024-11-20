pub(crate) mod auth;
pub(crate) mod grpc_message;
pub(crate) mod rate_limit;

use crate::configuration::{FailureMode, Service, ServiceType};
use crate::envoy::StatusCode;
use crate::operation_dispatcher::Operation;
use crate::runtime_action::RuntimeAction;
use crate::service::auth::{AuthService, AUTH_METHOD_NAME, AUTH_SERVICE_NAME};
use crate::service::grpc_message::{GrpcMessageRequest, GrpcMessageResponse};
use crate::service::rate_limit::{RateLimitService, RATELIMIT_METHOD_NAME, RATELIMIT_SERVICE_NAME};
use crate::service::TracingHeader::{Baggage, Traceparent, Tracestate};
use log::warn;
use protobuf::Message;
use proxy_wasm::hostcalls;
use proxy_wasm::types::Status::SerializationFailure;
use proxy_wasm::types::{BufferType, Bytes, MapType, Status};
use std::cell::OnceCell;
use std::rc::Rc;
use std::time::Duration;

#[derive(Default, Debug)]
pub struct GrpcService {
    service: Rc<Service>,
    name: &'static str,
    method: &'static str,
}

impl GrpcService {
    pub fn new(service: Rc<Service>) -> Self {
        match service.service_type {
            ServiceType::Auth => Self {
                service,
                name: AUTH_SERVICE_NAME,
                method: AUTH_METHOD_NAME,
            },
            ServiceType::RateLimit => Self {
                service,
                name: RATELIMIT_SERVICE_NAME,
                method: RATELIMIT_METHOD_NAME,
            },
        }
    }

    pub fn get_timeout(&self) -> Duration {
        self.service.timeout.0
    }

    pub fn get_service_type(&self) -> ServiceType {
        self.service.service_type.clone()
    }

    pub fn get_failure_mode(&self) -> FailureMode {
        self.service.failure_mode
    }

    fn endpoint(&self) -> &str {
        &self.service.endpoint
    }
    fn name(&self) -> &str {
        self.name
    }
    fn method(&self) -> &str {
        self.method
    }

    pub fn process_grpc_response(
        operation: Rc<Operation>,
        resp_size: usize,
    ) -> Result<GrpcResult, StatusCode> {
        let failure_mode = operation.get_failure_mode();
        if let Ok(Some(res_body_bytes)) =
            hostcalls::get_buffer(BufferType::GrpcReceiveBuffer, 0, resp_size)
        {
            match GrpcMessageResponse::new(&operation.get_service_type(), &res_body_bytes) {
                Ok(res) => match operation.get_service_type() {
                    ServiceType::Auth => AuthService::process_auth_grpc_response(res, failure_mode),
                    ServiceType::RateLimit => {
                        RateLimitService::process_ratelimit_grpc_response(res, failure_mode)
                    }
                },
                Err(e) => {
                    warn!(
                        "failed to parse grpc response body into GrpcMessageResponse message: {e}"
                    );
                    GrpcService::handle_error_on_grpc_response(failure_mode);
                    Err(StatusCode::InternalServerError)
                }
            }
        } else {
            warn!("failed to get grpc buffer or return data is null!");
            GrpcService::handle_error_on_grpc_response(failure_mode);
            Err(StatusCode::InternalServerError)
        }
    }

    pub fn handle_error_on_grpc_response(failure_mode: FailureMode) {
        match failure_mode {
            FailureMode::Deny => {
                hostcalls::send_http_response(500, vec![], Some(b"Internal Server Error.\n"))
                    .expect("failed to send_http_response 500");
            }
            FailureMode::Allow => {
                hostcalls::resume_http_request().expect("failed to resume_http_request")
            }
        }
    }

    pub fn get_service_type(&self) -> &ServiceType {
        &self.service.service_type
    }
}

pub struct GrpcResult {
    pub response_headers: Vec<(String, String)>,
}
impl GrpcResult {
    pub fn default() -> Self {
        Self {
            response_headers: Vec::new(),
        }
    }
    pub fn new(response_headers: Vec<(String, String)>) -> Self {
        Self { response_headers }
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

pub type GrpcMessageBuildFn = fn(action: &RuntimeAction) -> Option<GrpcMessageRequest>;

#[derive(Debug)]
pub struct GrpcServiceHandler {
    grpc_service: Rc<GrpcService>,
    header_resolver: Rc<HeaderResolver>,
    pub service_metrics: ServiceMetrics,
}

impl GrpcServiceHandler {
    pub fn new(
        grpc_service: Rc<GrpcService>,
        header_resolver: Rc<HeaderResolver>,
        service_metrics: ServiceMetrics,
    ) -> Self {
        Self {
            grpc_service,
            header_resolver,
            service_metrics,
        }
    }

    pub fn send(
        &self,
        get_map_values_bytes_fn: GetMapValuesBytesFn,
        grpc_call_fn: GrpcCallFn,
        message: GrpcMessageRequest,
        timeout: Duration,
    ) -> Result<u32, Status> {
        let msg = Message::write_to_bytes(&message).map_err(|e| {
            warn!("Failed to write protobuf message to bytes: {e:?}");
            SerializationFailure
        })?;
        let metadata = self
            .header_resolver
            .get(get_map_values_bytes_fn)
            .iter()
            .map(|(header, value)| (*header, value.as_slice()))
            .collect();

        grpc_call_fn(
            self.grpc_service.endpoint(),
            self.grpc_service.name(),
            self.grpc_service.method(),
            metadata,
            Some(&msg),
            timeout,
        )
    }
}

#[derive(Debug)]
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

#[derive(Debug)]
pub struct ServiceMetrics {
    ok_metric_id: u32,
    error_metric_id: u32,
    rejected_metric_id: u32,
    failure_mode_allowed_metric_id: u32,
}

impl ServiceMetrics {
    pub fn new(
        ok_metric_id: u32,
        error_metric_id: u32,
        rejected_metric_id: u32,
        failure_mode_allowed_metric_id: u32,
    ) -> Self {
        Self {
            ok_metric_id,
            error_metric_id,
            rejected_metric_id,
            failure_mode_allowed_metric_id,
        }
    }

    fn report(metric_id: u32, offset: i64) {
        if let Err(e) = hostcalls::increment_metric(metric_id, offset) {
            warn!("report metric {metric_id}, error: {e:?}");
        }
    }

    pub fn report_error(&self) {
        Self::report(self.error_metric_id, 1);
    }

    pub fn report_allowed_on_failure(&self) {
        Self::report(self.failure_mode_allowed_metric_id, 1);
    }

    pub fn report_ok(&self) {
        Self::report(self.ok_metric_id, 1);
    }

    pub fn report_rejected(&self) {
        Self::report(self.rejected_metric_id, 1);
    }
}
