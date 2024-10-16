use log::debug;
use proxy_wasm::hostcalls;
use proxy_wasm::types::Status;
use std::fmt::{Debug, Display, Formatter};

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
