pub(crate) mod auth;
pub(crate) mod grpc_message;
pub(crate) mod rate_limit;

use crate::configuration::action::Action;
use crate::configuration::{FailureMode, Service, ServiceType};
use crate::envoy::StatusCode;
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
    ) -> Result<(), StatusCode> {
        let failure_mode = operation.get_failure_mode();
        if let Some(res_body_bytes) =
            hostcalls::get_buffer(BufferType::GrpcReceiveBuffer, 0, resp_size).unwrap()
        {
            match GrpcMessageResponse::new(operation.get_service_type(), &res_body_bytes) {
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
            warn!("grpc response body is empty!");
            GrpcService::handle_error_on_grpc_response(failure_mode);
            Err(StatusCode::InternalServerError)
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

    pub fn get_service_type(&self) -> &ServiceType {
        &self.service.service_type
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
    fn(service_type: &ServiceType, action: &Action) -> Option<GrpcMessageRequest>;

pub struct GrpcServiceHandler {
    grpc_service: Rc<GrpcService>,
    header_resolver: Rc<HeaderResolver>,
    pub service_metrics: Rc<ServiceMetrics>,
}

impl GrpcServiceHandler {
    pub fn new(
        grpc_service: Rc<GrpcService>,
        header_resolver: Rc<HeaderResolver>,
        service_metrics: Rc<ServiceMetrics>,
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
        let msg = Message::write_to_bytes(&message).unwrap();
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

    pub fn get_service(&self) -> Rc<Service> {
        Rc::clone(&self.grpc_service.service)
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
