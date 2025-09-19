use std::collections::HashMap;
use std::sync::Arc;

use crate::v2::data::attribute::{AttributeValue, Path};
use crate::v2::data::attribute::{PropError, PropertyError};
use crate::v2::temp::GrpcRequest;
use log::warn;
use proxy_wasm::hostcalls;

pub trait Service {
    type Response;
    fn dispatch(&self, ctx: &mut ReqRespCtx, scope: String) -> usize;
    fn parse_message(&self, message: Vec<u8>) -> Self::Response;

    #[deprecated]
    fn request_message(&self, ctx: &mut ReqRespCtx, scope: String) -> GrpcRequest;
}

#[derive(Clone)]
pub struct ReqRespCtx {
    backend: Arc<dyn AttributeResolver>,
}

impl ReqRespCtx {
    pub fn new(backend: Arc<dyn AttributeResolver + 'static>) -> Self {
        Self { backend }
    }

    pub fn get_attribute<T: AttributeValue>(
        &self,
        path: impl Into<Path>,
    ) -> Result<Option<T>, PropertyError> {
        self.get_attribute_ref(&path.into())
    }

    pub fn get_attribute_ref<T: AttributeValue>(
        &self,
        path: &Path,
    ) -> Result<Option<T>, PropertyError> {
        let value = match *path.tokens() {
            ["source", "remote_address"] => self
                .remote_address()
                .map(|o| o.map(|s| s.as_bytes().to_vec())),
            ["auth", ..] => self.backend.get_attribute(&wasm_prop(&path.tokens())),
            _ => self.backend.get_attribute(path),
        };
        match value {
            Ok(Some(value)) => Ok(Some(T::parse(value).map_err(PropertyError::Parse)?)),
            Ok(None) => Ok(None),
            Err(e) => Err(PropertyError::Get(e)),
        }
    }

    pub fn get_attribute_map(&self, path: &Path) -> Result<HashMap<String, String>, PropertyError> {
        match *path.tokens() {
            ["request", "headers"] => {
                match self
                    .backend
                    .get_attribute_map(proxy_wasm::types::MapType::HttpRequestHeaders)
                {
                    Ok(map) => Ok(map),
                    Err(err) => Err(PropertyError::Get(err)),
                }
            }
            _ => Err(PropertyError::Get(PropError::new(format!(
                "Unknown map requested: {}",
                path
            )))),
        }
    }

    fn remote_address(&self) -> Result<Option<String>, PropError> {
        // Ref https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_conn_man/headers#x-forwarded-for
        // Envoy sets source.address to the trusted client address AND port.
        match self.backend.get_attribute(&"source.address".into())? {
            None => {
                warn!("source.address property not found");
                Err(PropError::new("source.address not found".to_string()))
            }
            Some(host_vec) => {
                let source_address: String = AttributeValue::parse(host_vec)?;
                let split_address = source_address.split(':').collect::<Vec<_>>();
                Ok(Some(split_address[0].to_string()))
            }
        }
    }
}

pub trait AttributeResolver: Send + Sync {
    fn get_attribute(&self, path: &Path) -> Result<Option<Vec<u8>>, PropError>;
    fn get_attribute_map(
        &self,
        map_type: proxy_wasm::types::MapType,
    ) -> Result<HashMap<String, String>, PropError>;
}

struct ProxyWasmHost;

impl AttributeResolver for ProxyWasmHost {
    fn get_attribute(&self, path: &Path) -> Result<Option<Vec<u8>>, PropError> {
        match hostcalls::get_property(path.tokens()) {
            Ok(data) => Ok(data),
            // Err(Status::BadArgument) => Ok(PendingValue),
            Err(e) => Err(PropError::new(format!(
                "failed to get property: {path}: {e:?}"
            ))),
        }
    }

    fn get_attribute_map(
        &self,
        map_type: proxy_wasm::types::MapType,
    ) -> Result<HashMap<String, String>, PropError> {
        match hostcalls::get_map(map_type) {
            Ok(map) => Ok(map.into_iter().collect()),
            Err(err) => Err(PropError::new(format!("Error getting host map: {err:?}"))),
        }
    }
}

pub fn wasm_prop(tokens: &[&str]) -> Path {
    let mut flat_attr = "filter_state.wasm\\.kuadrant\\.".to_string();
    flat_attr.push_str(tokens.join("\\.").as_str());
    flat_attr.as_str().into()
}

#[cfg(test)]
pub mod tests {
    use std::collections::HashMap;

    use crate::v2::{
        data::attribute::Path, data::attribute::PropError, kuadrant::AttributeResolver,
    };

    #[derive(Default)]
    pub struct MockWasmHost {
        properties: HashMap<Path, Vec<u8>>,
        maps: HashMap<String, HashMap<String, String>>,
    }

    impl MockWasmHost {
        pub fn new() -> Self {
            Self {
                properties: HashMap::new(),
                maps: HashMap::new(),
            }
        }

        pub fn with_property(mut self, path: Path, value: Vec<u8>) -> Self {
            self.properties.insert(path, value);
            self
        }

        pub fn with_map(
            mut self,
            map_name: String,
            map: std::collections::HashMap<String, String>,
        ) -> Self {
            self.maps.insert(map_name, map);
            self
        }
    }

    impl AttributeResolver for MockWasmHost {
        fn get_attribute(&self, path: &Path) -> Result<Option<Vec<u8>>, PropError> {
            match self.properties.get(path) {
                Some(value) => Ok(Some(value.clone())),
                None => Ok(None),
            }
        }

        fn get_attribute_map(
            &self,
            map_type: proxy_wasm::types::MapType,
        ) -> Result<HashMap<String, String>, PropError> {
            let map_key = match map_type {
                proxy_wasm::types::MapType::HttpRequestHeaders => "request.headers",
                _ => {
                    return Err(PropError::new(format!(
                        "MockWasmHost does not support map type: {:?}",
                        map_type
                    )))
                }
            };

            match self.maps.get(map_key) {
                Some(map) => Ok(map.clone()),
                None => Err(PropError::new(format!(
                    "MockWasmHost does not have map: {}",
                    map_key
                ))),
            }
        }
    }
}
