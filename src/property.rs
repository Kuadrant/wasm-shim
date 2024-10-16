use crate::property_path::Path;
use log::debug;
use proxy_wasm::hostcalls;
use proxy_wasm::types::Status;

fn remote_address() -> Result<Option<Vec<u8>>, Status> {
    Ok(None)
}

fn host_get_property(property: &str) -> Result<Option<Vec<u8>>, Status> {
    let path = Path::from(property);
    debug!(
        "get_property:  selector: {} path: {:?}",
        property,
        path.tokens()
    );
    hostcalls::get_property(path.tokens())
}

pub fn get_property(property: &str) -> Result<Option<Vec<u8>>, Status> {
    match property {
        "kuadrant.remote_address" => remote_address(),
        _ => host_get_property(property),
    }
}
