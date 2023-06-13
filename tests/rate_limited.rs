use proxy_wasm_test_framework::tester;
use proxy_wasm_test_framework::types::{Action, BufferType, LogLevel, MapType, ReturnType};

#[test]
fn it_loads() {
    let args = tester::MockSettings {
        wasm_path: "target/wasm32-unknown-unknown/release/wasm_shim.wasm".to_string(),
        quiet: false,
        allow_unexpected: false,
    };
    let mut module = tester::mock(args).unwrap();

    module
        .call_start()
        .execute_and_expect(ReturnType::None)
        .unwrap();

    let root_context = 1;
    let cfg = r#"{
        "failure_mode_deny": true,
        "rate_limit_policies": []
    }"#;

    module
        .call_proxy_on_context_create(root_context, 0)
        .expect_log(Some(LogLevel::Info), Some("set_root_context #1"))
        .execute_and_expect(ReturnType::None)
        .unwrap();
    module
        .call_proxy_on_configure(root_context, 0)
        .expect_log(Some(LogLevel::Info), Some("on_configure #1"))
        .expect_get_buffer_bytes(Some(BufferType::PluginConfiguration))
        .returning(Some(cfg))
        .expect_log(Some(LogLevel::Info), Some("plugin config parsed: PluginConfiguration { rate_limit_policies: [], failure_mode_deny: true }"))
        .execute_and_expect(ReturnType::Bool(true))
        .unwrap();

    let http_context = 2;
    module
        .call_proxy_on_context_create(http_context, root_context)
        .expect_log(Some(LogLevel::Info), Some("create_http_context #2"))
        .execute_and_expect(ReturnType::None)
        .unwrap();

    module
        .call_proxy_on_request_headers(http_context, 0, false)
        .expect_log(Some(LogLevel::Info), Some("on_http_request_headers #2"))
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some(":authority"))
        .returning(None)
        .expect_log(
            Some(LogLevel::Info),
            Some("context #2: Allowing request to pass because zero descriptors generated"),
        )
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    module
        .call_proxy_on_response_headers(http_context, 0, false)
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();
}
