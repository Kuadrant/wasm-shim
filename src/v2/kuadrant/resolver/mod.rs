use crate::v2::data::attribute::{AttributeError, Path};
use crate::services::ServiceError;
use std::time::Duration;

mod wasm_host;
pub use wasm_host::ProxyWasmHost;

#[cfg(test)]
mod mock;

#[cfg(test)]
pub use mock::MockWasmHost;

pub trait AttributeResolver: Send + Sync {
    fn get_attribute(&self, path: &Path) -> Result<Option<Vec<u8>>, AttributeError>;
    fn get_attribute_map(
        &self,
        map_type: proxy_wasm::types::MapType,
    ) -> Result<Vec<(String, String)>, AttributeError>;
    fn set_attribute(&self, path: &Path, value: &[u8]) -> Result<(), AttributeError>;
    fn set_attribute_map(
        &self,
        map_type: proxy_wasm::types::MapType,
        value: Vec<(&str, &str)>,
    ) -> Result<(), AttributeError>;
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
