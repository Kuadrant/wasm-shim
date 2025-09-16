use crate::util::common::wasm_module;
use proxy_wasm_test_framework::tester;
use proxy_wasm_test_framework::types::{Action, BufferType, LogLevel, MapType, ReturnType};
use serial_test::serial;

pub mod util;

#[test]
#[serial]
fn it_runs_next_action_on_failure_when_failuremode_is_allow() {
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
                "endpoint": "limitador-cluster",
                "failureMode": "deny",
                "timeout": "5s"
            },
            "limitador-unreachable": {
                "type": "ratelimit",
                "endpoint": "unreachable-cluster",
                "failureMode": "allow",
                "timeout": "5s"
            }
        },
        "actionSets": [
        {
            "name": "some-name",
            "routeRuleConditions": {
                "hostnames": ["example.com"]
            },
            "actions": [
            {
                "service": "limitador-unreachable",
                "scope": "a",
                "conditionalData": [
                {
                    "data": [
                        {
                            "expression": {
                                "key": "l",
                                "value": "1"
                            }
                        }
                    ]
                }]
            },
            {
                "service": "limitador",
                "scope": "a",
                "conditionalData": [
                {
                    "data": [
                        {
                            "expression": {
                                "key": "l",
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

    let first_call_token_id = 42;
    module
        .call_proxy_on_request_headers(http_context, 0, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_request_headers"))
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some(":authority"))
        .returning(Some("example.com"))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 action_set selected some-name"),
        )
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 send_grpc_request: unreachable-cluster envoy.service.ratelimit.v3.RateLimitService ShouldRateLimit 5s"),
        )
        // retrieving tracing headers
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("traceparent"))
        .returning(None)
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("tracestate"))
        .returning(None)
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("baggage"))
        .returning(None)
        .expect_grpc_call(
            Some("unreachable-cluster"),
            Some("envoy.service.ratelimit.v3.RateLimitService"),
            Some("ShouldRateLimit"),
            Some(&[1, 0, 0, 0, 10, 0, 0, 0, 19, 0, 0, 0, 58, 97, 117, 116, 104, 111, 114, 105, 116, 121, 0, 117, 110, 114, 101, 97, 99, 104, 97, 98, 108, 101, 45, 99, 108, 117, 115, 116, 101, 114, 0]),
            None,
            Some(5000),
        )
        .returning(Ok(first_call_token_id))
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    let status_code = 14;
    let second_call_token_id = 43;
    module
        .proxy_on_grpc_close(http_context, first_call_token_id as i32, status_code)
        .expect_log(
            Some(LogLevel::Debug),
            Some(format!("#2 on_grpc_call_response: received gRPC call response: token: {first_call_token_id}, status: {status_code}").as_str()),
        )
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 send_grpc_request: limitador-cluster envoy.service.ratelimit.v3.RateLimitService ShouldRateLimit 5s"),
        )
        .expect_grpc_call(
            Some("limitador-cluster"),
            Some("envoy.service.ratelimit.v3.RateLimitService"),
            Some("ShouldRateLimit"),
            Some(&[1, 0, 0, 0, 10, 0, 0, 0, 17, 0, 0, 0, 58, 97, 117, 116, 104, 111, 114, 105, 116, 121, 0, 108, 105, 109, 105, 116, 97, 100, 111, 114, 45, 99, 108, 117, 115, 116, 101, 114, 0]),
            None,
            Some(5000),
        )
        .returning(Ok(second_call_token_id))
        .execute_and_expect(ReturnType::None)
        .unwrap();

    let grpc_response: [u8; 2] = [8, 1];
    module
        .call_proxy_on_grpc_receive(
            http_context,
            second_call_token_id as i32,
            grpc_response.len() as i32,
        )
        .expect_log(
            Some(LogLevel::Debug),
            Some(format!("#2 on_grpc_call_response: received gRPC call response: token: {second_call_token_id}, status: 0").as_str()),
        )
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(&grpc_response))
        .expect_log(
            Some(LogLevel::Debug),
            Some("process_response(rl): received OK response"),
        )
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
fn it_stops_on_failure_when_failuremode_is_deny() {
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
                "endpoint": "limitador-cluster",
                "failureMode": "deny",
                "timeout": "5s"
            },
            "limitador-unreachable": {
                "type": "ratelimit",
                "endpoint": "unreachable-cluster",
                "failureMode": "deny",
                "timeout": "5s"
            }
        },
        "actionSets": [
        {
            "name": "some-name",
            "routeRuleConditions": {
                "hostnames": ["example.com"]
            },
            "actions": [
            {
                "service": "limitador-unreachable",
                "scope": "a",
                "conditionalData": [
                {
                    "data": [
                        {
                            "expression": {
                                "key": "l",
                                "value": "1"
                            }
                        }
                    ]
                }]
            },
            {
                "service": "limitador",
                "scope": "a",
                "conditionalData": [
                {
                    "data": [
                        {
                            "expression": {
                                "key": "l",
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
        .returning(Some("example.com"))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 action_set selected some-name"),
        )
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 send_grpc_request: unreachable-cluster envoy.service.ratelimit.v3.RateLimitService ShouldRateLimit 5s"),
        )
        // retrieving tracing headers
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("traceparent"))
        .returning(None)
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("tracestate"))
        .returning(None)
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("baggage"))
        .returning(None)
        .expect_grpc_call(
            Some("unreachable-cluster"),
            Some("envoy.service.ratelimit.v3.RateLimitService"),
            Some("ShouldRateLimit"),
            Some(&[1, 0, 0, 0, 10, 0, 0, 0, 19, 0, 0, 0, 58, 97, 117, 116, 104, 111, 114, 105, 116, 121, 0, 117, 110, 114, 101, 97, 99, 104, 97, 98, 108, 101, 45, 99, 108, 117, 115, 116, 101, 114, 0]),
            None,
            Some(5000),
        )
        .returning(Ok(42))
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    let status_code = 14;
    module
        .proxy_on_grpc_close(http_context, 42, status_code)
        .expect_log(
            Some(LogLevel::Debug),
            Some(format!("#2 on_grpc_call_response: received gRPC call response: token: 42, status: {status_code}").as_str()),
        )
        .expect_send_local_response(Some(500), None, None, None)
        .execute_and_expect(ReturnType::None)
        .unwrap();
}
