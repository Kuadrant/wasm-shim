use crate::data::attribute::{AttributeError, Path};
use crate::services::ServiceError;
use std::time::Duration;

#[cfg(test)]
mod mock;

#[cfg(test)]
pub use mock::MockWasmHost;

pub trait AttributeResolver: Send + Sync {
    fn get_attribute(&self, path: &Path) -> Result<Option<Vec<u8>>, AttributeError>;
    fn get_request_headers(&self) -> Result<Vec<(String, String)>, AttributeError>;
    fn get_response_headers(&self) -> Result<Vec<(String, String)>, AttributeError>;
    fn get_request_header_value(&self, key: &str) -> Result<Option<String>, AttributeError>;
    fn set_attribute(&self, path: &Path, value: &[u8]) -> Result<(), AttributeError>;
    fn set_request_headers(&self, value: Vec<(&str, &str)>) -> Result<(), AttributeError>;
    fn set_response_headers(&self, value: Vec<(&str, &str)>) -> Result<(), AttributeError>;
    fn get_http_request_body(
        &self,
        start: usize,
        max_size: usize,
    ) -> Result<Option<Vec<u8>>, AttributeError>;
    fn get_http_response_body(
        &self,
        start: usize,
        max_size: usize,
    ) -> Result<Option<Vec<u8>>, AttributeError>;
    fn dispatch_grpc_call(
        &self,
        upstream_name: &str,
        service_name: &str,
        method: &str,
        headers: Vec<(&str, &[u8])>,
        message: Vec<u8>,
        timeout: Duration,
    ) -> Result<u32, ServiceError>;
    fn get_grpc_response(&self, response_size: usize) -> Result<Vec<u8>, ServiceError>;
    fn send_http_reply(
        &self,
        status_code: u32,
        headers: Vec<(&str, &str)>,
        body: Option<&[u8]>,
    ) -> Result<(), ServiceError>;
}
