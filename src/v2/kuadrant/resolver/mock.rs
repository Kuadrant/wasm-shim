use super::AttributeResolver;
use crate::v2::data::attribute::{AttributeError, Path};
use crate::v2::services::ServiceError;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

#[derive(Default)]
pub struct MockWasmHost {
    properties: Mutex<HashMap<Path, Vec<u8>>>,
    maps: Mutex<HashMap<String, Vec<(String, String)>>>,
    pending_properties: Vec<Path>,
}

impl MockWasmHost {
    pub fn new() -> Self {
        Self {
            properties: Mutex::new(HashMap::new()),
            maps: Mutex::new(HashMap::new()),
            pending_properties: Vec::new(),
        }
    }

    pub fn with_property(self, path: Path, value: Vec<u8>) -> Self {
        self.properties
            .lock()
            .expect("properties mutex poisoned")
            .insert(path, value);
        self
    }

    pub fn with_map(self, map_name: String, map: Vec<(String, String)>) -> Self {
        self.maps
            .lock()
            .expect("maps mutex poisoned")
            .insert(map_name, map);
        self
    }

    pub fn with_pending_property(mut self, path: Path) -> Self {
        self.pending_properties.push(path);
        self
    }

    pub fn get_property(&self, path: &Path) -> Option<Vec<u8>> {
        self.properties
            .lock()
            .expect("properties mutex poisoned")
            .get(path)
            .cloned()
    }

    pub fn get_map(&self, map_name: &str) -> Option<Vec<(String, String)>> {
        self.maps
            .lock()
            .expect("maps mutex poisoned")
            .get(map_name)
            .cloned()
    }
}

impl AttributeResolver for MockWasmHost {
    fn get_attribute(&self, path: &Path) -> Result<Option<Vec<u8>>, AttributeError> {
        if self.pending_properties.contains(path) {
            return Err(AttributeError::NotAvailable(format!(
                "Property {} is pending",
                path
            )));
        }
        Ok(self.get_property(path))
    }

    fn get_attribute_map(
        &self,
        map_type: proxy_wasm::types::MapType,
    ) -> Result<Vec<(String, String)>, AttributeError> {
        let map_key = match map_type {
            proxy_wasm::types::MapType::HttpRequestHeaders => "request.headers",
            proxy_wasm::types::MapType::HttpResponseHeaders => "response.headers",
            _ => {
                return Err(AttributeError::Retrieval(format!(
                    "MockWasmHost does not support map type: {:?}",
                    map_type
                )))
            }
        };

        match self.get_map(map_key) {
            Some(map) => Ok(map),
            None => Err(AttributeError::Retrieval(format!(
                "MockWasmHost does not have map: {}",
                map_key
            ))),
        }
    }

    fn set_attribute(&self, path: &Path, value: &[u8]) -> Result<(), AttributeError> {
        self.properties
            .lock()
            .expect("properties mutex poisoned")
            .insert(path.clone(), value.to_vec());
        Ok(())
    }

    fn set_attribute_map(
        &self,
        map_type: proxy_wasm::types::MapType,
        value: Vec<(&str, &str)>,
    ) -> Result<(), AttributeError> {
        let map_key = match map_type {
            proxy_wasm::types::MapType::HttpRequestHeaders => "request.headers",
            proxy_wasm::types::MapType::HttpResponseHeaders => "response.headers",
            _ => {
                return Err(AttributeError::Set(format!(
                    "MockWasmHost does not support map type: {:?}",
                    map_type
                )))
            }
        };

        let owned_map: Vec<(String, String)> = value
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        self.maps
            .lock()
            .expect("maps mutex poisoned")
            .insert(map_key.to_string(), owned_map);
        Ok(())
    }

    fn dispatch_grpc_call(
        &self,
        _upstream_name: &str,
        _service_name: &str,
        _method: &str,
        _headers: Vec<(&str, &[u8])>,
        _message: Vec<u8>,
        _timeout: Duration,
    ) -> Result<u32, ServiceError> {
        // todo(refactor): mock returns a fake token_id
        // in real tests, we'd need to store the message and allow retrieving responses
        Ok(42)
    }
}
