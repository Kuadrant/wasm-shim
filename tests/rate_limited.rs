use proxy_wasm_test_framework::tester;
use proxy_wasm_test_framework::types::ReturnType;

#[test]
fn it_works() {
    let args = tester::MockSettings {
        wasm_path: "target/wasm32-unknown-unknown/release/wasm_shim.wasm".to_string(),
        quiet: false,
        allow_unexpected: false,
    };
    let mut hello_world_test = tester::mock(args)?;

    hello_world_test
        .call_start()
        .execute_and_expect(ReturnType::None)?;
}