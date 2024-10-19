use log::debug;
use log::warn;
use proxy_wasm::hostcalls;
use proxy_wasm::types::Status;
use std::fmt::{Display, Formatter};

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

fn host_get_property(path: &Path) -> Result<Option<Vec<u8>>, Status> {
    debug!("get_property: path: {:?}", path);
    hostcalls::get_property(path.tokens())
}

pub fn get_property(path: &Path) -> Result<Option<Vec<u8>>, Status> {
    match path.tokens()[..] {
        ["source", "remote_address"] => remote_address(),
        _ => host_get_property(path),
    }
}

#[derive(Debug, Clone)]
pub struct Path {
    tokens: Vec<String>,
}

impl Display for Path {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            self.tokens
                .iter()
                .map(|t| t.replace('.', "\\."))
                .collect::<Vec<String>>()
                .join(".")
        )
    }
}

impl From<&str> for Path {
    fn from(value: &str) -> Self {
        let mut token = String::new();
        let mut tokens: Vec<String> = Vec::new();
        let mut chars = value.chars();
        while let Some(ch) = chars.next() {
            match ch {
                '.' => {
                    tokens.push(token);
                    token = String::new();
                }
                '\\' => {
                    if let Some(next) = chars.next() {
                        token.push(next);
                    }
                }
                _ => token.push(ch),
            }
        }
        tokens.push(token);

        Self { tokens }
    }
}

impl Path {
    pub fn tokens(&self) -> Vec<&str> {
        self.tokens.iter().map(String::as_str).collect()
    }
}

#[cfg(test)]
mod test {
    use crate::data::property::Path;
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
}
