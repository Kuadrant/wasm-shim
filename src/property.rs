use crate::property_path::Path;
use log::debug;
use log::warn;
use proxy_wasm::hostcalls;
use proxy_wasm::types::MapType;
use proxy_wasm::types::Status;

fn remote_address() -> Result<Option<Vec<u8>>, Status> {
    // From https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_conn_man/headers#x-forwarded-for
    // If use_remote_address is set to true, Envoy sets the x-envoy-external-address header to the trusted client address.
    debug!("get_map_value(MapType::HttpRequestHeaders, x-envoy-external-address");
    match hostcalls::get_map_value(MapType::HttpRequestHeaders, "x-envoy-external-address")? {
        None => {
            debug!("x-envoy-external-address property not found");
            // When the incoming request does not have XFF header,
            // envoy does not set x-envoy-external-address regardless of the xff_num_trusted_hops
            // value. The request is considered originated internally.
            // For this case, the trusted client address is the source IP address
            // of the immediate downstream node’s connection to Envoy
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
        Some(trusted_client_address) => Ok(Some(trusted_client_address.as_bytes().to_vec())),
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
