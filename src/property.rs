use crate::property_path::Path;
use log::debug;
use log::warn;
use proxy_wasm::hostcalls;
use proxy_wasm::types::Status;

fn remote_address() -> Result<Option<Vec<u8>>, Status> {
    // Ref https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_conn_man/headers#x-forwarded-for
    // Envoy sets source.address to the trusted client address AND port.
    match host_get_property("source.address")? {
        None => {
            warn!("source.address property not found");
            Err(Status::BadArgument)
        }
        Some(host_vec) => match String::from_utf8(host_vec) {
            Err(e) => {
                warn!("source.address property value not string: {}", e);
                Err(Status::BadArgument)
            }
            Ok(source_address) => {
                let split_address = source_address.split(':').collect::<Vec<_>>();
                Ok(Some(split_address[0].as_bytes().to_vec()))
            }
        },
    }
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
        "source.remote_address" => remote_address(),
        _ => host_get_property(property),
    }
}
