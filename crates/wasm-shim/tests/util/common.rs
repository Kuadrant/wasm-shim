use std::path::Path;

use proxy_wasm_test_framework::types::LogLevel;

pub const LOG_LEVEL: LogLevel = LogLevel::Warn;

#[allow(clippy::unwrap_used)]
pub fn wasm_module() -> String {
    let wasm_file = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/wasm32-wasip1/release/wasm_shim.wasm");
    assert!(
        wasm_file.exists(),
        "Run `cargo build --release --target=wasm32-wasip1` first"
    );
    wasm_file.to_str().unwrap().to_string()
}

pub fn json_escape_cel(cel: &str) -> String {
    cel.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('"', r#"\""#)
}

pub fn auth_check_request_cel(scope: &str) -> String {
    r#"envoy.service.auth.v3.CheckRequest {
        attributes: envoy.service.auth.v3.AttributeContext {
            request: envoy.service.auth.v3.AttributeContext.Request {
                time: request.time,
                http: envoy.service.auth.v3.AttributeContext.HttpRequest {
                    host: request.host,
                    method: request.method,
                    scheme: request.scheme,
                    path: request.path,
                    protocol: request.protocol,
                    headers: request.headers
                }
            },
            destination: envoy.service.auth.v3.AttributeContext.Peer {
                address: envoy.config.core.v3.Address {
                    socket_address: envoy.config.core.v3.SocketAddress {
                        address: destination.address,
                        port_value: uint(destination.port)
                    }
                }
            },
            source: envoy.service.auth.v3.AttributeContext.Peer {
                address: envoy.config.core.v3.Address {
                    socket_address: envoy.config.core.v3.SocketAddress {
                        address: source.address,
                        port_value: uint(source.port)
                    }
                }
            },
            context_extensions: {"host": "__SCOPE__"},
            metadata_context: envoy.config.core.v3.Metadata{}
        }
    }"#
    .replace("__SCOPE__", scope)
}
