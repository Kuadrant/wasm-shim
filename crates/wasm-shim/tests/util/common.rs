use std::path::PathBuf;

use proxy_wasm_test_framework::types::LogLevel;

pub const LOG_LEVEL: LogLevel = LogLevel::Warn;

#[allow(clippy::unwrap_used)]
pub fn wasm_module() -> String {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let wasm_file = workspace_root.join("target/wasm32-wasip1/release/wasm_shim.wasm");
    assert!(
        wasm_file.exists(),
        "Run `cargo build --release --target=wasm32-wasip1` first"
    );
    wasm_file.to_str().unwrap().to_string()
}
