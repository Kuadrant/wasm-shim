use crate::v2::data::attribute::Path;
use crate::v2::kuadrant;
use log::debug;
use log::warn;
use proxy_wasm::types::Status;
use std::collections::HashMap;
use std::fmt::{Debug, Display, Formatter};

#[deprecated]
fn remote_address() -> Result<Option<Vec<u8>>, Status> {
    // Ref https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_conn_man/headers#x-forwarded-for
    // Envoy sets source.address to the trusted client address AND port.
    match host_get_property(&"source.address".into())? {
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

#[cfg(test)]
pub(super) fn host_get_property(path: &Path) -> Result<Option<Vec<u8>>, Status> {
    debug!("get_property: {:?}", path);
    match test::TEST_PROPERTY_VALUE.take() {
        None => Err(Status::NotFound),
        Some((expected_path, data)) => {
            assert_eq!(&expected_path, path);
            Ok(Some(data))
        }
    }
}

#[cfg(test)]
pub fn host_get_map(path: &Path) -> Result<HashMap<String, String>, String> {
    debug!("host_get_map: {:?}", path);
    match test::TEST_MAP_VALUE.take() {
        None => Err(format!("Unknown map requested {path:?}")),
        Some((expected_path, data)) => {
            assert_eq!(&expected_path, path);
            Ok(data)
        }
    }
}

#[cfg(test)]
pub fn host_set_property(path: Path, value: Option<&[u8]>) -> Result<(), Status> {
    debug!("set_property: {:?}", path);
    let data = value.map(|bytes| bytes.to_vec()).unwrap_or_default();
    test::TEST_PROPERTY_VALUE.set(Some((path, data)));
    Ok(())
}

#[cfg(not(test))]
pub fn host_get_map(path: &Path) -> Result<HashMap<String, String>, String> {
    match *path.tokens() {
        ["request", "headers"] => {
            match proxy_wasm::hostcalls::get_map(proxy_wasm::types::MapType::HttpRequestHeaders) {
                Ok(map) => Ok(map.into_iter().collect()),
                Err(status) => Err(format!("Error get request.headers: {status:?}")),
            }
        }
        _ => Err(format!("Unknown map requested {path:?}")),
    }
}

#[cfg(not(test))]
pub(super) fn host_get_property(path: &Path) -> Result<Option<Vec<u8>>, Status> {
    debug!("get_property: {path:?}");
    proxy_wasm::hostcalls::get_property(path.tokens())
}

#[cfg(not(test))]
pub(super) fn host_set_property(path: Path, value: Option<&[u8]>) -> Result<(), Status> {
    debug!("set_property: {path:?}");
    proxy_wasm::hostcalls::set_property(path.tokens(), value)
}

pub(super) fn get_property(path: &Path) -> Result<Option<Vec<u8>>, Status> {
    match *path.tokens() {
        ["source", "remote_address"] => remote_address(),
        ["auth", ..] => host_get_property(&kuadrant::wasm_prop(path.tokens().as_slice())),
        _ => host_get_property(path),
    }
}

pub(super) fn set_property(path: Path, value: Option<&[u8]>) -> Result<(), Status> {
    host_set_property(path, value)
}

#[cfg(test)]
pub mod test {
    use super::*;
    use crate::v2::kuadrant::wasm_prop;
    use std::cell::Cell;

    thread_local!(
        pub static TEST_PROPERTY_VALUE: Cell<Option<(Path, Vec<u8>)>> = const { Cell::new(None) };
        pub static TEST_MAP_VALUE: Cell<Option<(Path, HashMap<String, String>)>> =
            const { Cell::new(None) };
    );

    #[test]
    fn path_tokenizes_with_escaping_basic() {
        let path: Path = r"one\.two..three\\\\.four\\\.\five.".into();
        assert_eq!(
            path.tokens(),
            vec!["one.two", "", r"three\\", r"four\.five", ""]
        );
    }

    #[test]
    fn path_tokenizes_with_escaping_ends_with_separator() {
        let path: Path = r"one.".into();
        assert_eq!(path.tokens(), vec!["one", ""]);
    }

    #[test]
    fn path_tokenizes_with_escaping_ends_with_escape() {
        let path: Path = r"one\".into();
        assert_eq!(path.tokens(), vec!["one"]);
    }

    #[test]
    fn path_tokenizes_with_escaping_starts_with_separator() {
        let path: Path = r".one".into();
        assert_eq!(path.tokens(), vec!["", "one"]);
    }

    #[test]
    fn path_tokenizes_with_escaping_starts_with_escape() {
        let path: Path = r"\one".into();
        assert_eq!(path.tokens(), vec!["one"]);
    }

    #[test]
    fn flat_wasm_prop() {
        let path = wasm_prop(&["auth", "identity", "anonymous"]);
        assert_eq!(path.tokens().len(), 2);
        assert_eq!(
            *path.tokens(),
            ["filter_state", "wasm.kuadrant.auth.identity.anonymous"]
        );
    }
}
