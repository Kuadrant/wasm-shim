use std::time::Duration;

use super::AttributeResolver;
use crate::v2::data::attribute::{AttributeError, Path};
use crate::v2::services::ServiceError;
use proxy_wasm::hostcalls;

pub struct ProxyWasmHost;

impl AttributeResolver for ProxyWasmHost {
    fn get_attribute(&self, path: &Path) -> Result<Option<Vec<u8>>, AttributeError> {
        match hostcalls::get_property(path.tokens()) {
            Ok(data) => Ok(data),
            Err(proxy_wasm::types::Status::BadArgument) => Err(AttributeError::NotAvailable(
                format!("Property {path} not available in current request phase"),
            )),
            Err(e) => Err(AttributeError::Retrieval(format!(
                "failed to get property: {path}: {e:?}"
            ))),
        }
    }

    fn get_attribute_map(
        &self,
        map_type: proxy_wasm::types::MapType,
    ) -> Result<Vec<(String, String)>, AttributeError> {
        match hostcalls::get_map(map_type) {
            Ok(map) if map.is_empty() => Err(AttributeError::NotAvailable(format!(
                "Map {:?} not available in current phase",
                map_type
            ))),
            Ok(map) => Ok(map),
            Err(err) => Err(AttributeError::Retrieval(format!(
                "Error getting host map: {err:?}"
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
                format!("Map {:?} not available in current phase", map_type),
            )),
            Err(err) => Err(AttributeError::Set(format!("Error setting map: {err:?}"))),
        }
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
        hostcalls::dispatch_grpc_call(
            upstream_name,
            service_name,
            method,
            headers,
            Some(&message),
            timeout,
        )
        .map_err(|e| ServiceError::DispatchFailed(format!("{e:?}")))
    }
}
