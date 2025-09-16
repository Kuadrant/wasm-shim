use crate::util::common::wasm_module;
use proxy_wasm_test_framework::tester;
use proxy_wasm_test_framework::types::{Action, BufferType, LogLevel, MapType, ReturnType};
use serial_test::serial;

pub mod util;

#[test]
#[serial]
fn it_sets_grpc_authority_header() {
    let args = tester::MockSettings {
        wasm_path: wasm_module(),
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
        "services": {
            "limitador": {
                "type": "ratelimit",
                "endpoint": "outbound|8081||limitador-limitador.kuadrant-system.svc.cluster.local",
                "failureMode": "deny",
                "timeout": "5s"
            }
        },
        "actionSets": [
        {
            "name": "some-name",
            "routeRuleConditions": {
                "hostnames": ["*.com"]
            },
            "actions": [
            {
                "service": "limitador",
                "scope": "RLS-domain",
                "conditionalData": [
                {
                    "data": [
                        {
                            "static": {
                                "key": "admin",
                                "value": "1"
                            }
                        }
                    ]
                }]
            }]
        }]
    }"#;

    module
        .call_proxy_on_context_create(root_context, 0)
        .expect_log(Some(LogLevel::Info), Some("#1 set_root_context"))
        .execute_and_expect(ReturnType::None)
        .unwrap();

    module
        .call_proxy_on_configure(root_context, 0)
        .expect_log(Some(LogLevel::Info), Some("#1 on_configure"))
        .expect_get_buffer_bytes(Some(BufferType::PluginConfiguration))
        .returning(Some(cfg.as_bytes()))
        .expect_log(Some(LogLevel::Info), None)
        .execute_and_expect(ReturnType::Bool(true))
        .unwrap();

    let http_context = 2;
    module
        .call_proxy_on_context_create(http_context, root_context)
        .expect_log(Some(LogLevel::Debug), Some("#2 create_http_context"))
        .execute_and_expect(ReturnType::None)
        .unwrap();

    module
        .call_proxy_on_request_headers(http_context, 0, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_request_headers"))
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some(":authority"))
        .returning(Some("cars.toystore.com"))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 action_set selected some-name"),
        )
        .expect_log(Some(LogLevel::Debug), Some("#2 send_grpc_request: outbound|8081||limitador-limitador.kuadrant-system.svc.cluster.local envoy.service.ratelimit.v3.RateLimitService ShouldRateLimit 5s"))
        // retrieving tracing headers
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("traceparent"))
        .returning(None)
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("tracestate"))
        .returning(None)
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("baggage"))
        .returning(None)
        .expect_grpc_call(
            Some("outbound|8081||limitador-limitador.kuadrant-system.svc.cluster.local"),
            Some("envoy.service.ratelimit.v3.RateLimitService"),
            Some("ShouldRateLimit"),
            Some(&[
                1, 0, 0, 0,                                          // one header
                10, 0, 0, 0,                                         // key length = 10
                53, 0, 0, 0,                                         // value length = 53
                58, 97, 117, 116, 104, 111, 114, 105, 116, 121, 0,   // ":authority" + NUL
                108, 105, 109, 105, 116, 97, 100, 111, 114, 45, 108,
                105, 109, 105, 116, 97, 100, 111, 114, 46, 107, 117,
                97, 100, 114, 97, 110, 116, 45, 115, 121, 115, 116,
                101, 109, 46, 115, 118, 99, 46, 99, 108, 117, 115,
                116, 101, 114, 46, 108, 111, 99, 97, 108, 0           // "limitador-limitador.kuadrant-system.svc.cluster.local" + NUL
            ]),
            None,
            Some(5000),
        )
        .returning(Ok(42))
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();
}
