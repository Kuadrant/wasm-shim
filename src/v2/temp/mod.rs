use std::time::Duration;

// GrpcRequest contains the information required to make a Grpc Call
pub struct GrpcRequest {
    upstream_name: String,
    service_name: String,
    method_name: String,
    timeout: Duration,
    message: Option<Vec<u8>>,
}

impl GrpcRequest {
    pub fn new(
        upstream_name: &str,
        service_name: &str,
        method_name: &str,
        timeout: Duration,
        message: Option<Vec<u8>>,
    ) -> Self {
        Self {
            upstream_name: upstream_name.to_owned(),
            service_name: service_name.to_owned(),
            method_name: method_name.to_owned(),
            timeout,
            message,
        }
    }

    pub fn upstream_name(&self) -> &str {
        &self.upstream_name
    }

    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    pub fn method_name(&self) -> &str {
        &self.method_name
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    pub fn message(&self) -> Option<&[u8]> {
        self.message.as_deref()
    }
}
