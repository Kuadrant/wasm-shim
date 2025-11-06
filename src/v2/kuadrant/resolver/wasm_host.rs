use std::time::Duration;

use log::{debug, error};

use super::AttributeResolver;
use crate::v2::data::attribute::{AttributeError, Path};
use crate::v2::services::ServiceError;
use proxy_wasm::hostcalls;

pub struct ProxyWasmHost;

impl AttributeResolver for ProxyWasmHost {
    fn get_attribute(&self, path: &Path) -> Result<Option<Vec<u8>>, AttributeError> {
        debug!("Getting property: `{}`", path);
        match hostcalls::get_property(path.tokens()) {
            Ok(data) => Ok(data),
            Err(proxy_wasm::types::Status::BadArgument) => Err(AttributeError::NotAvailable(
                format!("Property `{path}` not available in current request phase"),
            )),
            Err(e) => Err(AttributeError::Retrieval(format!(
                "failed to get property: `{path}`: {e:?}"
            ))),
        }
    }

    fn get_attribute_map(
        &self,
        map_type: proxy_wasm::types::MapType,
    ) -> Result<Vec<(String, String)>, AttributeError> {
        debug!("Getting map: `{:?}`", map_type);
        match hostcalls::get_map(map_type) {
            Ok(map) if map.is_empty() => Err(AttributeError::NotAvailable(format!(
                "Map `{:?}` not available in current phase",
                map_type
            ))),
            Ok(map) => Ok(map),
            Err(err) => Err(AttributeError::Retrieval(format!(
                "Error getting host map: {err:?}"
            ))),
        }
    }

    fn set_attribute(&self, path: &Path, value: &[u8]) -> Result<(), AttributeError> {
        debug!("Setting property: `{}`", path);
        match hostcalls::set_property(path.tokens(), Some(value)) {
            Ok(_) => Ok(()),
            Err(err) => Err(AttributeError::Set(format!(
                "Failed to set property `{}`: {:?}",
                path, err
            ))),
        }
    }

    fn set_attribute_map(
        &self,
        map_type: proxy_wasm::types::MapType,
        value: Vec<(&str, &str)>,
    ) -> Result<(), AttributeError> {
        match hostcalls::set_map(map_type, value) {
            Ok(_) => Ok(()),
            Err(proxy_wasm::types::Status::BadArgument) => Err(AttributeError::NotAvailable(
                format!("Map `{:?}` not available in current phase", map_type),
            )),
            Err(err) => Err(AttributeError::Set(format!("Error setting map: {err:?}"))),
        }
    }

    fn get_http_response_body(
        &self,
        start: usize,
        max_size: usize,
    ) -> Result<Option<proxy_wasm::types::Bytes>, AttributeError> {
        hostcalls::get_buffer(
            proxy_wasm::types::BufferType::HttpResponseBody,
            start,
            max_size,
        )
        .map_err(|e| {
            AttributeError::Retrieval(format!("Error getting http response body buffer: {e:?}"))
        })
    }

    fn dispatch_grpc_call(
        &self,
        upstream_name: &str,
        service_name: &str,
        method: &str,
        headers: Vec<(&str, &[u8])>,
        message: Vec<u8>,
        timeout: Duration,
    ) -> Result<u32, ServiceError> {
        debug!(
            "Dispatching gRPC call to {}/{}.{}, timeout: {:?}",
            upstream_name, service_name, method, timeout
        );
        match hostcalls::dispatch_grpc_call(
            upstream_name,
            service_name,
            method,
            headers,
            Some(&message),
            timeout,
        ) {
            Ok(token_id) => {
                debug!("gRPC call dispatched successfully, token_id: {}", token_id);
                Ok(token_id)
            }
            Err(e) => {
                error!(
                    "Failed to dispatch gRPC call to {}/{}.{}: {:?}",
                    upstream_name, service_name, method, e
                );
                Err(ServiceError::Dispatch(format!("{e:?}")))
            }
        }
    }

    fn get_grpc_response(&self, response_size: usize) -> Result<Vec<u8>, ServiceError> {
        if response_size == 0 {
            return Err(ServiceError::Retrieval(
                "Received response with size 0".to_string(),
            ));
        }
        debug!("Getting gRPC response, size: {} bytes", response_size);
        hostcalls::get_buffer(
            proxy_wasm::types::BufferType::GrpcReceiveBuffer,
            0,
            response_size,
        )
        .map_err(|e| ServiceError::Retrieval(format!("Failed to get gRPC response: {:?}", e)))?
        .ok_or_else(|| ServiceError::Retrieval("No gRPC response body available".to_string()))
    }

    fn send_http_reply(
        &self,
        status_code: u32,
        headers: Vec<(&str, &str)>,
        body: Option<&[u8]>,
    ) -> Result<(), ServiceError> {
        debug!("Sending local reply, status code: {}", status_code);
        hostcalls::send_http_response(status_code, headers, body)
            .map_err(|e| ServiceError::Dispatch(format!("Failed to send HTTP reply: {:?}", e)))
    }
}
