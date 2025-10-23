use crate::v2::data::attribute::{AttributeError, Path};

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
    fn set_attribute_map(
        &self,
        map_type: proxy_wasm::types::MapType,
        value: Vec<(&str, &str)>,
    ) -> Result<(), AttributeError>;
}
