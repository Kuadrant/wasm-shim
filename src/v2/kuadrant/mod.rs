use crate::data::{PropError, PropertyError};
use crate::v2::data::attribute::{AttributeValue, Path};
use crate::v2::temp::GrpcRequest;
use log::warn;
use proxy_wasm::hostcalls;
use proxy_wasm::types::{Bytes, Status};

pub trait Service {
    type Response;
    fn dispatch(&self, ctx: &mut ReqRespCtx) -> usize;
    fn parse_message(&self, message: Vec<u8>) -> Self::Response;

    #[deprecated]
    fn request_message(&self, ctx: &mut ReqRespCtx) -> GrpcRequest;
}

pub struct ReqRespCtx {
    backend: Box<dyn AttributeResolver>,
}

impl ReqRespCtx {
    pub fn get_attribute<T: AttributeValue>(
        &self,
        attribute_name: &str,
    ) -> Result<Option<T>, PropertyError> {
        let path: Path = attribute_name.into();
        let value = match *path.tokens() {
            ["source", "remote_address"] => self
                .remote_address()
                .map(|o| o.map(|s| s.as_bytes().to_vec())),
            ["auth", ..] => self.backend.get_attribute(wasm_prop(&path.tokens())),
            _ => self.backend.get_attribute(path),
        };
        match value {
            Ok(Some(value)) => Ok(Some(T::parse(value).map_err(PropertyError::Parse)?)),
            Ok(None) => Ok(None),
            Err(e) => Err(PropertyError::Get(e)),
        }
    }

    fn remote_address(&self) -> Result<Option<String>, PropError> {
        // Ref https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_conn_man/headers#x-forwarded-for
        // Envoy sets source.address to the trusted client address AND port.
        match self.backend.get_attribute("source.address".into())? {
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

trait AttributeResolver {
    fn get_attribute(&self, path: Path) -> Result<Option<Vec<u8>>, PropError>;
}

struct ProxyWasmHost;

impl AttributeResolver for ProxyWasmHost {
    fn get_attribute(&self, path: Path) -> Result<Option<Vec<u8>>, PropError> {
        match hostcalls::get_property(path.tokens()) {
            Ok(data) => Ok(data),
            // Err(Status::BadArgument) => Ok(PendingValue),
            Err(e) => Err(PropError::new(format!(
                "failed to get property: {path}: {e:?}"
            ))),
        }
    }
}

pub fn wasm_prop(tokens: &[&str]) -> Path {
    let mut flat_attr = "filter_state.wasm\\.kuadrant\\.".to_string();
    flat_attr.push_str(tokens.join("\\.").as_str());
    flat_attr.as_str().into()
}
