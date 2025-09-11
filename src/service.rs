pub(crate) mod auth;
pub(crate) mod rate_limit;

use crate::configuration::{FailureMode, Service, ServiceType};
use crate::envoy::{HeaderValue, HeaderValueOption, StatusCode};
use crate::service::auth::{AUTH_METHOD_NAME, AUTH_SERVICE_NAME};
use crate::service::rate_limit::{
    KUADRANT_CHECK_RATELIMIT_METHOD_NAME, KUADRANT_RATELIMIT_SERVICE_NAME,
    KUADRANT_REPORT_RATELIMIT_METHOD_NAME, RATELIMIT_METHOD_NAME, RATELIMIT_SERVICE_NAME,
};
use crate::service::TracingHeader::{Baggage, Traceparent, Tracestate};
use crate::v2::temp::GrpcRequest;
use protobuf::RepeatedField;
use proxy_wasm::types::Bytes;
use std::cell::OnceCell;
use std::rc::Rc;
use std::time::Duration;

pub(super) mod errors {
    use crate::data::{EvaluationError, Expression};
    use crate::v2::data::attribute::PropertyError;
    use protobuf::ProtobufError;
    use std::fmt::{Debug, Display, Formatter};

    #[derive(Debug)]
    pub enum BuildMessageError {
        Evaluation(Box<EvaluationError>),
        Property(PropertyError),
        Serialization(ProtobufError),
        UnsupportedDataType {
            /// Box the contents of expressoin to avoid large error variants
            expression: Box<Expression>,
            got: String,
            want: String,
        },
    }

    impl BuildMessageError {
        pub fn new_unsupported_data_type_err(e: Expression, got: String, want: String) -> Self {
            BuildMessageError::UnsupportedDataType {
                expression: Box::new(e),
                got,
                want,
            }
        }
    }

    impl From<EvaluationError> for BuildMessageError {
        fn from(e: EvaluationError) -> Self {
            BuildMessageError::Evaluation(e.into())
        }
    }

    impl Display for BuildMessageError {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            match self {
                BuildMessageError::Evaluation(e) => {
                    write!(f, "BuildMessageError::Evaluation {{ {e:?} }}")
                }
                BuildMessageError::Property(e) => {
                    write!(f, "BuildMessageError::Property {{ {e:?} }}")
                }
                BuildMessageError::Serialization(e) => {
                    write!(f, "BuildMessageError::Serialization {{ {e:?} }}")
                }
                BuildMessageError::UnsupportedDataType {
                    expression,
                    got,
                    want,
                } => {
                    write!(
                        f,
                        "BuildMessageError::UnsupportedDataType {{ expression: {expression:?}; got: {got}; want: {want}}}"
                    )
                }
            }
        }
    }

    #[derive(Debug)]
    pub enum ProcessGrpcMessageError {
        Protobuf(ProtobufError),
        Property(PropertyError),
        EmptyResponse,
        UnsupportedField,
    }

    impl From<ProtobufError> for ProcessGrpcMessageError {
        fn from(e: ProtobufError) -> Self {
            ProcessGrpcMessageError::Protobuf(e)
        }
    }
}

#[derive(Default, Debug, PartialEq, Clone)]
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
            ServiceType::RateLimitCheck => Self {
                service,
                name: KUADRANT_RATELIMIT_SERVICE_NAME,
                method: KUADRANT_CHECK_RATELIMIT_METHOD_NAME,
            },
            ServiceType::RateLimitReport => Self {
                service,
                name: KUADRANT_RATELIMIT_SERVICE_NAME,
                method: KUADRANT_REPORT_RATELIMIT_METHOD_NAME,
            },
        }
    }

    pub fn get_timeout(&self) -> Duration {
        self.service.timeout.0
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
    pub fn build_request(&self, message: Option<Vec<u8>>) -> Option<GrpcRequest> {
        message.map(|msg| {
            GrpcRequest::new(
                self.endpoint(),
                self.name(),
                self.method(),
                self.get_timeout(),
                Some(msg),
            )
        })
    }
}

pub struct IndexedGrpcRequest {
    index: usize,
    request: GrpcRequest,
}

impl IndexedGrpcRequest {
    pub(crate) fn new(index: usize, request: GrpcRequest) -> Self {
        Self { index, request }
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn request(self) -> GrpcRequest {
        self.request
    }
}

pub type Headers = Vec<(String, String)>;

pub fn from_envoy_rl_headers(headers: RepeatedField<HeaderValue>) -> Headers {
    headers
        .into_iter()
        .map(|header| (header.key, header.value))
        .collect()
}

pub fn from_envoy_headers(headers: &[HeaderValueOption]) -> Headers {
    headers
        .iter()
        .map(|header| {
            let hv = header.get_header();
            (hv.key.to_owned(), hv.value.to_owned())
        })
        .collect()
}

#[derive(Debug)]
pub struct DirectResponse {
    status_code: u32,
    response_headers: Headers,
    body: String,
}

impl DirectResponse {
    pub fn new(status_code: u32, response_headers: Headers, body: String) -> Self {
        Self {
            status_code,
            response_headers,
            body,
        }
    }

    pub fn new_internal_server_error() -> Self {
        Self {
            status_code: StatusCode::InternalServerError as u32,
            response_headers: Vec::default(),
            body: "Internal Server Error.\n".to_string(),
        }
    }

    pub fn status_code(&self) -> u32 {
        self.status_code
    }

    pub fn headers(&self) -> Vec<(&str, &str)> {
        self.response_headers
            .iter()
            .map(|(header, value)| (header.as_str(), value.as_str()))
            .collect()
    }

    pub fn body(&self) -> &str {
        self.body.as_str()
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

    pub fn get_with_ctx<T: proxy_wasm::traits::HttpContext>(
        &self,
        ctx: &T,
    ) -> &Vec<(&'static str, Bytes)> {
        self.headers.get_or_init(|| {
            let mut headers = Vec::new();
            for header in TracingHeader::all() {
                match ctx.get_http_request_header_bytes((*header).as_str()) {
                    Ok(Some(value)) => headers.push(((*header).as_str(), value)),
                    Ok(None) => (),
                    Err(status) => log::error!("Error getting header: {:?} ", status),
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

#[cfg(test)]
mod test {
    use super::*;
    use proxy_wasm::traits::Context;
    use proxy_wasm::types::Status;
    use std::collections::HashMap;

    struct MockHost {
        headers: HashMap<&'static str, Bytes>,
    }

    impl MockHost {
        pub fn new(headers: HashMap<&'static str, Bytes>) -> Self {
            Self { headers }
        }
    }

    impl Context for MockHost {}

    impl proxy_wasm::traits::HttpContext for MockHost {
        fn get_http_request_header_bytes(&self, name: &str) -> Result<Option<Bytes>, Status> {
            Ok(self.headers.get(name).map(|b| b.to_owned()))
        }
    }

    #[test]
    fn read_headers() {
        let header_resolver = HeaderResolver::new();

        let headers: Vec<(&str, Bytes)> = vec![("traceparent", b"xyz".to_vec())];
        let mock_host = MockHost::new(headers.iter().cloned().collect::<HashMap<_, _>>());

        let resolver_headers = header_resolver.get_with_ctx(&mock_host);

        headers.iter().zip(resolver_headers.iter()).for_each(
            |((header_one, value_one), (header_two, value_two))| {
                assert_eq!(header_one, header_two);
                assert_eq!(value_one, value_two);
            },
        )
    }
}
