use crate::v2::data::attribute::{AttributeError, Path};
use std::collections::HashMap;

mod wasm_host;

#[cfg(test)]
mod mock;

#[cfg(test)]
pub use mock::MockWasmHost;

pub trait AttributeResolver: Send + Sync {
    fn get_attribute(&self, path: &Path) -> Result<Option<Vec<u8>>, AttributeError>;
    fn get_attribute_map(
        &self,
        map_type: proxy_wasm::types::MapType,
    ) -> Result<HashMap<String, String>, AttributeError>;
    fn set_attribute_map(
        &self,
        map_type: proxy_wasm::types::MapType,
        value: HashMap<String, String>,
    ) -> Result<(), AttributeError>;
}
