use std::path::Path;

pub fn wasm_module() -> String {
    let wasm_file = Path::new("target/wasm32-unknown-unknown/release/wasm_shim.wasm");
    assert!(
        wasm_file.exists(),
        "Run `cargo build --release --target=wasm32-unknown-unknown` first"
    );
    wasm_file.to_str().unwrap().to_string()
}
