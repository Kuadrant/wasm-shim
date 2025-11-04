use super::AttributeResolver;
use crate::v2::data::attribute::{AttributeError, Path};
use crate::v2::services::ServiceError;
use proxy_wasm::types::Bytes;
use std::collections::HashMap;
use std::time::Duration;

#[derive(Default)]
pub struct MockWasmHost {
    properties: HashMap<Path, Vec<u8>>,
    maps: HashMap<String, Vec<(String, String)>>,
    pending_properties: Vec<Path>,
    response_body: Option<Bytes>,
}

impl MockWasmHost {
    pub fn new() -> Self {
        Self {
            properties: HashMap::new(),
            maps: HashMap::new(),
            pending_properties: Vec::new(),
            response_body: None,
        }
    }

    pub fn with_property(mut self, path: Path, value: Vec<u8>) -> Self {
        self.properties.insert(path, value);
        self
    }

    pub fn with_map(mut self, map_name: String, map: Vec<(String, String)>) -> Self {
        self.maps.insert(map_name, map);
        self
    }

    pub fn with_pending_property(mut self, path: Path) -> Self {
        self.pending_properties.push(path);
        self
    }

    pub fn with_response_body(mut self, bytes: &[u8]) -> Self {
        self.response_body = Some(bytes.to_vec());
        self
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
        match self.properties.get(path) {
            Some(value) => Ok(Some(value.clone())),
            None => Ok(None),
        }
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

        match self.maps.get(map_key) {
            Some(map) => Ok(map.clone()),
            None => Err(AttributeError::Retrieval(format!(
                "MockWasmHost does not have map: {}",
                map_key
            ))),
        }
    }

    fn set_attribute_map(
        &self,
        _map_type: proxy_wasm::types::MapType,
        _value: Vec<(&str, &str)>,
    ) -> Result<(), AttributeError> {
        Ok(())
    }

    fn get_http_response_body(
        &self,
        start: usize,
        max_size: usize,
    ) -> Result<Option<Bytes>, AttributeError> {
        match &self.response_body {
            Some(body) => {
                let buf_end_index = std::cmp::min(start + max_size, body.len());
                let mut dst: Bytes = vec![0; buf_end_index];
                assert!(start <= buf_end_index, "messed up with the indexes!");
                dst.clone_from_slice(&body[start..buf_end_index]);
                Ok(Some(dst))
            }
            None => Ok(None),
        }
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
