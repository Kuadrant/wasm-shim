use proxy_wasm_test_framework::tester;
use proxy_wasm_test_framework::types::{Action, BufferType, LogLevel, MapType, ReturnType};
use serial_test::serial;
use std::path::Path;

fn wasm_module() -> String {
    let wasm_file = Path::new("target/wasm32-unknown-unknown/release/wasm_shim.wasm");
    assert!(
        wasm_file.exists(),
        "Run `cargo build --release --target=wasm32-unknown-unknown` first"
    );
    wasm_file.to_str().unwrap().to_string()
}

#[test]
#[serial]
fn it_loads() {
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
        "extensions": {},
        "policies": []
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
            Some("#2 allowing request to pass because zero descriptors generated"),
        )
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    module
        .call_proxy_on_response_headers(http_context, 0, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_response_headers"))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();
}

#[test]
#[serial]
fn it_limits() {
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
        "extensions": {
            "limitador": {
                "type": "ratelimit",
                "endpoint": "limitador-cluster",
                "failureMode": "deny"
            }
        },
        "policies": [
        {
            "name": "some-name",
            "domain": "RLS-domain",
            "hostnames": ["*.toystore.com", "example.com"],
            "rules": [
            {
                "conditions": [
                    {
                        "allOf": [
                        {
                            "selector": "request.url_path",
                            "operator": "startswith",
                            "value": "/admin/toy"
                        },
                        {
                            "selector": "request.host",
                            "operator": "eq",
                            "value": "cars.toystore.com"
                        },
                        {
                            "selector": "request.method",
                            "operator": "eq",
                            "value": "POST"
                        }]
                    }
                ],
                "actions": [
                    {
                        "extension": "limitador",
                        "data": [
                        {
                            "static": {
                              "key": "admin",
                              "value": "1"
                            }
                        }]
                    }
                ]
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
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("traceparent"))
        .returning(None)
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("tracestate"))
        .returning(None)
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("baggage"))
        .returning(None)
        .expect_get_property(Some(vec!["request", "url_path"]))
        .returning(Some("/admin/toy".as_bytes()))
        .expect_get_property(Some(vec!["request", "host"]))
        .returning(Some("cars.toystore.com".as_bytes()))
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some("POST".as_bytes()))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 ratelimitpolicy selected some-name"),
        )
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property:  selector: request.url_path path: [\"request\", \"url_path\"]"),
        )
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property:  selector: request.host path: [\"request\", \"host\"]"),
        )
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property:  selector: request.method path: [\"request\", \"method\"]"),
        )
        .expect_grpc_call(
            Some("limitador-cluster"),
            Some("envoy.service.ratelimit.v3.RateLimitService"),
            Some("ShouldRateLimit"),
            Some(&[0, 0, 0, 0]),
            Some(&[
                10, 10, 82, 76, 83, 45, 100, 111, 109, 97, 105, 110, 18, 12, 10, 10, 10, 5, 97,
                100, 109, 105, 110, 18, 1, 49, 24, 1,
            ]),
            Some(5000),
        )
        .returning(Some(42))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 initiated gRPC call (id# 42)"),
        )
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    let grpc_response: [u8; 2] = [8, 1];
    module
        .call_proxy_on_grpc_receive(http_context, 42, grpc_response.len() as i32)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 on_grpc_call_response: received gRPC call response: token: 42, status: 0"),
        )
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(&grpc_response))
        .execute_and_expect(ReturnType::None)
        .unwrap();

    module
        .call_proxy_on_response_headers(http_context, 0, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_response_headers"))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();
}

#[test]
#[serial]
fn it_passes_additional_headers() {
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
        "extensions": {
            "limitador": {
                "type": "ratelimit",
                "endpoint": "limitador-cluster",
                "failureMode": "deny"
            }
        },
        "policies": [
        {
            "name": "some-name",
            "domain": "RLS-domain",
            "hostnames": ["*.toystore.com", "example.com"],
            "rules": [
            {
                "conditions": [
                    {
                        "allOf": [
                        {
                            "selector": "request.url_path",
                            "operator": "startswith",
                            "value": "/admin/toy"
                        },
                        {
                            "selector": "request.host",
                            "operator": "eq",
                            "value": "cars.toystore.com"
                        },
                        {
                            "selector": "request.method",
                            "operator": "eq",
                            "value": "POST"
                        }]
                    }
                ],
                "actions": [
                    {
                        "extension": "limitador",
                        "data": [
                          {
                            "static": {
                              "key": "admin",
                              "value": "1"
                            }
                          }
                        ]
                    }
                ]
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
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("traceparent"))
        .returning(None)
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("tracestate"))
        .returning(None)
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("baggage"))
        .returning(None)
        .expect_get_property(Some(vec!["request", "url_path"]))
        .returning(Some("/admin/toy".as_bytes()))
        .expect_get_property(Some(vec!["request", "host"]))
        .returning(Some("cars.toystore.com".as_bytes()))
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some("POST".as_bytes()))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 ratelimitpolicy selected some-name"),
        )
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property:  selector: request.url_path path: [\"request\", \"url_path\"]"),
        )
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property:  selector: request.host path: [\"request\", \"host\"]"),
        )
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property:  selector: request.method path: [\"request\", \"method\"]"),
        )
        .expect_grpc_call(
            Some("limitador-cluster"),
            Some("envoy.service.ratelimit.v3.RateLimitService"),
            Some("ShouldRateLimit"),
            Some(&[0, 0, 0, 0]),
            Some(&[
                10, 10, 82, 76, 83, 45, 100, 111, 109, 97, 105, 110, 18, 12, 10, 10, 10, 5, 97,
                100, 109, 105, 110, 18, 1, 49, 24, 1,
            ]),
            Some(5000),
        )
        .returning(Some(42))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 initiated gRPC call (id# 42)"),
        )
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    let grpc_response: [u8; 45] = [
        8, 1, 26, 18, 10, 4, 116, 101, 115, 116, 18, 10, 115, 111, 109, 101, 32, 118, 97, 108, 117,
        101, 26, 21, 10, 5, 111, 116, 104, 101, 114, 18, 12, 104, 101, 97, 100, 101, 114, 32, 118,
        97, 108, 117, 101,
    ];
    module
        .call_proxy_on_grpc_receive(http_context, 42, grpc_response.len() as i32)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 on_grpc_call_response: received gRPC call response: token: 42, status: 0"),
        )
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(&grpc_response))
        .execute_and_expect(ReturnType::None)
        .unwrap();

    module
        .call_proxy_on_response_headers(http_context, 0, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_response_headers"))
        .expect_add_header_map_value(
            Some(MapType::HttpResponseHeaders),
            Some("test"),
            Some("some value"),
        )
        .expect_add_header_map_value(
            Some(MapType::HttpResponseHeaders),
            Some("other"),
            Some("header value"),
        )
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();
}

#[test]
#[serial]
fn it_rate_limits_with_empty_conditions() {
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
        "extensions": {
            "limitador": {
                "type": "ratelimit",
                "endpoint": "limitador-cluster",
                "failureMode": "deny"
            }
        },
        "policies": [
        {
            "name": "some-name",
            "domain": "RLS-domain",
            "hostnames": ["*.com"],
            "rules": [
            {
                "actions": [
                {
                    "extension": "limitador",
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
        .returning(Some("a.com"))
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("traceparent"))
        .returning(None)
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("tracestate"))
        .returning(None)
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("baggage"))
        .returning(None)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 ratelimitpolicy selected some-name"),
        )
        .expect_grpc_call(
            Some("limitador-cluster"),
            Some("envoy.service.ratelimit.v3.RateLimitService"),
            Some("ShouldRateLimit"),
            Some(&[0, 0, 0, 0]),
            Some(&[
                10, 10, 82, 76, 83, 45, 100, 111, 109, 97, 105, 110, 18, 12, 10, 10, 10, 5, 97,
                100, 109, 105, 110, 18, 1, 49, 24, 1,
            ]),
            Some(5000),
        )
        .returning(Some(42))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 initiated gRPC call (id# 42)"),
        )
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    let grpc_response: [u8; 2] = [8, 1];
    module
        .call_proxy_on_grpc_receive(http_context, 42, grpc_response.len() as i32)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 on_grpc_call_response: received gRPC call response: token: 42, status: 0"),
        )
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(&grpc_response))
        .execute_and_expect(ReturnType::None)
        .unwrap();

    module
        .call_proxy_on_response_headers(http_context, 0, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_response_headers"))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();
}

#[test]
#[serial]
fn it_does_not_rate_limits_when_selector_does_not_exist_and_misses_default_value() {
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
        "extensions": {
            "limitador": {
                "type": "ratelimit",
                "endpoint": "limitador-cluster",
                "failureMode": "deny"
            }
        },
        "policies": [
        {
            "name": "some-name",
            "domain": "RLS-domain",
            "hostnames": ["*.com"],
            "rules": [
            {
                "actions": [
                    {
                        "extension": "limitador",
                        "data": [
                            {
                                "selector": {
                                    "selector": "unknown.path"
                                }
                            }
                        ]
                    }
                ]
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
        .returning(Some("a.com"))
        .expect_get_property(Some(vec!["unknown", "path"]))
        .returning(None)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 ratelimitpolicy selected some-name"),
        )
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property:  selector: unknown.path path: Path { tokens: [\"unknown\", \"path\"] }"),
        )
        .expect_log(
            Some(LogLevel::Debug),
            Some("build_single_descriptor: selector not found: unknown.path"),
        )
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 process_policy: empty descriptors"),
        )
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    module
        .call_proxy_on_response_headers(http_context, 0, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_response_headers"))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();
}
