use crate::data::{PropError, PropertyError};
use crate::v2::data::attribute::{AttributeValue, Path};
use crate::v2::temp::GrpcRequest;
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
        match *path.tokens() {
            ["source", "remote_address"] => todo!(),
            ["auth", ..] => todo!(),
            _ => match self.backend.get_attribute(path) {
                Ok(Some(value)) => Ok(Some(T::parse(value).map_err(PropertyError::Parse)?)),
                Ok(None) => Ok(None),
                Err(e) => Err(PropertyError::Get(e)),
            },
        }
    }
}

trait AttributeResolver {
    fn get_attribute(&self, path: Path) -> Result<Option<Vec<u8>>, PropError>;
}

struct ProxyWasmHost {}

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
