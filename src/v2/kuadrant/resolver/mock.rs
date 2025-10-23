use super::AttributeResolver;
use crate::v2::data::attribute::{AttributeError, Path};
use std::collections::HashMap;

#[derive(Default)]
pub struct MockWasmHost {
    properties: HashMap<Path, Vec<u8>>,
    maps: HashMap<String, Vec<(String, String)>>,
    pending_properties: Vec<Path>,
}

impl MockWasmHost {
    pub fn new() -> Self {
        Self {
            properties: HashMap::new(),
            maps: HashMap::new(),
            pending_properties: Vec::new(),
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
        _value: Vec<(String, String)>,
    ) -> Result<(), AttributeError> {
        Ok(())
    }
}
