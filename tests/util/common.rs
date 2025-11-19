use std::path::Path;

#[allow(clippy::unwrap_used)]
pub fn wasm_module() -> String {
    let wasm_file = Path::new("target/wasm32-wasip1/release/wasm_shim.wasm");
    assert!(
        wasm_file.exists(),
        "Run `cargo build --release --target=wasm32-wasip1` first"
    );
    wasm_file.to_str().unwrap().to_string()
}
