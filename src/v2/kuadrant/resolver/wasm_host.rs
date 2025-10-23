use super::AttributeResolver;
use crate::v2::data::attribute::{AttributeError, Path};
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
            Ok(map) => Ok(map),
            Err(err) => Err(AttributeError::Retrieval(format!(
                "Error getting host map: {err:?}"
            ))),
        }
    }

    fn set_attribute_map(
        &self,
        map_type: proxy_wasm::types::MapType,
        value: Vec<(String, String)>,
    ) -> Result<(), AttributeError> {
        match hostcalls::set_map(
            map_type,
            value
                .iter()
                .map(|(s1, s2)| (s1.as_str(), s2.as_str()))
                .collect(),
        ) {
            Ok(_) => Ok(()),
            Err(err) => Err(AttributeError::Set(format!("Error setting map: {err:?}"))),
        }
    }
}
