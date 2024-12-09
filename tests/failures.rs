use crate::util::common::wasm_module;
use proxy_wasm_test_framework::tester;
use proxy_wasm_test_framework::types::Status as TestStatus;
use proxy_wasm_test_framework::types::{Action, BufferType, LogLevel, MapType, ReturnType};
use serial_test::serial;

pub mod util;

#[test]
#[serial]
fn it_fails_on_first_action_grpc_call() {
    // this usually happens when target service is not registered on host
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
            "mistyped-service": {
                "type": "ratelimit",
                "endpoint": "does-not-exist",
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
                "service": "mistyped-service",
                "scope": "a",
                "data": [
                    {
                        "expression": {
                            "key": "limit_to_be_activated",
                            "value": "1"
                        }
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
        .returning(Some("example.com"))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 action_set selected some-name"),
        )
        // retrieving tracing headers
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("traceparent"))
        .returning(None)
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("tracestate"))
        .returning(None)
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("baggage"))
        .returning(None)
        .expect_grpc_call(
            Some("does-not-exist"),
            Some("envoy.service.ratelimit.v3.RateLimitService"),
            Some("ShouldRateLimit"),
            Some(&[0, 0, 0, 0]),
            Some(&[
                10, 1, 97, 18, 28, 10, 26, 10, 21, 108, 105, 109, 105, 116, 95, 116, 111, 95, 98,
                101, 95, 97, 99, 116, 105, 118, 97, 116, 101, 100, 18, 1, 49, 24, 1,
            ]),
            Some(5000),
        )
        .returning(Err(TestStatus::ParseFailure))
        .expect_log(
            Some(LogLevel::Error),
            Some("OperationError { status: ParseFailure, failure_mode: Deny }"),
        )
        .expect_log(
            Some(LogLevel::Warn),
            Some("OperationError Status: ParseFailure"),
        )
        .expect_send_local_response(
            Some(500),
            Some("Internal Server Error.\n"),
            Some(vec![]),
            Some(-1),
        )
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();
}

#[test]
#[serial]
fn it_fails_on_second_action_grpc_call() {
    // this usually happens when target service is not registered on host
    // testing error handling on the second call as the error handling is implemented
    // differently from the first call
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
            "mistyped-service": {
                "type": "ratelimit",
                "endpoint": "does-not-exist",
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
                "service": "limitador",
                "scope": "a",
                "data": [
                    {
                        "expression": {
                            "key": "limit_to_be_activated",
                            "value": "1"
                        }
                    }
                ]
            },
            {
                "service": "mistyped-service",
                "scope": "a",
                "data": [
                    {
                        "expression": {
                            "key": "limit_to_be_activated",
                            "value": "1"
                        }
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
        .returning(Some("example.com"))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 action_set selected some-name"),
        )
        // retrieving tracing headers
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("traceparent"))
        .returning(None)
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("tracestate"))
        .returning(None)
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("baggage"))
        .returning(None)
        .expect_grpc_call(
            Some("limitador-cluster"),
            Some("envoy.service.ratelimit.v3.RateLimitService"),
            Some("ShouldRateLimit"),
            Some(&[0, 0, 0, 0]),
            Some(&[
                10, 1, 97, 18, 28, 10, 26, 10, 21, 108, 105, 109, 105, 116, 95, 116, 111, 95, 98,
                101, 95, 97, 99, 116, 105, 118, 97, 116, 101, 100, 18, 1, 49, 24, 1,
            ]),
            Some(5000),
        )
        .returning(Ok(42))
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
        .expect_grpc_call(
            Some("does-not-exist"),
            Some("envoy.service.ratelimit.v3.RateLimitService"),
            Some("ShouldRateLimit"),
            Some(&[0, 0, 0, 0]),
            Some(&[
                10, 1, 97, 18, 28, 10, 26, 10, 21, 108, 105, 109, 105, 116, 95, 116, 111, 95, 98,
                101, 95, 97, 99, 116, 105, 118, 97, 116, 101, 100, 18, 1, 49, 24, 1,
            ]),
            Some(5000),
        )
        .returning(Err(TestStatus::ParseFailure))
        .expect_log(
            Some(LogLevel::Error),
            Some("OperationError { status: ParseFailure, failure_mode: Deny }"),
        )
        .expect_send_local_response(
            Some(500),
            Some("Internal Server Error.\n"),
            Some(vec![]),
            Some(-1),
        )
        .execute_and_expect(ReturnType::None)
        .unwrap();
}

#[test]
#[serial]
fn it_fails_on_first_action_grpc_response() {
    // this usually happens when target service is registered on host but unreachable
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
                "data": [
                    {
                        "expression": {
                            "key": "limit_to_be_activated",
                            "value": "1"
                        }
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
        .returning(Some("example.com"))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 action_set selected some-name"),
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
            Some(&[0, 0, 0, 0]),
            Some(&[
                10, 1, 97, 18, 28, 10, 26, 10, 21, 108, 105, 109, 105, 116, 95, 116, 111, 95, 98,
                101, 95, 97, 99, 116, 105, 118, 97, 116, 101, 100, 18, 1, 49, 24, 1,
            ]),
            Some(5000),
        )
        .returning(Ok(42))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 initiated gRPC call (id# 42)"),
        )
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    let status_code = 14;
    module
        .proxy_on_grpc_close(http_context, 42, status_code)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 on_grpc_call_response: received gRPC call response: token: 42, status: 14"),
        )
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(&[]))
        .expect_send_local_response(
            Some(500),
            Some("Internal Server Error.\n"),
            Some(vec![]),
            Some(-1),
        )
        .execute_and_expect(ReturnType::None)
        .unwrap();
}

#[test]
#[serial]
fn it_fails_on_second_action_grpc_response() {
    // this usually happens when target service is registered on host but unreachable
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
                "service": "limitador",
                "scope": "a",
                "data": [
                    {
                        "expression": {
                            "key": "limit_to_be_activated",
                            "value": "1"
                        }
                    }
                ]
            },
            {
                "service": "limitador-unreachable",
                "scope": "a",
                "data": [
                    {
                        "expression": {
                            "key": "limit_to_be_activated",
                            "value": "1"
                        }
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
    let first_call_token_id = 42;
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
        // retrieving tracing headers
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("traceparent"))
        .returning(None)
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("tracestate"))
        .returning(None)
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("baggage"))
        .returning(None)
        .expect_grpc_call(
            Some("limitador-cluster"),
            Some("envoy.service.ratelimit.v3.RateLimitService"),
            Some("ShouldRateLimit"),
            Some(&[0, 0, 0, 0]),
            Some(&[
                10, 1, 97, 18, 28, 10, 26, 10, 21, 108, 105, 109, 105, 116, 95, 116, 111, 95, 98,
                101, 95, 97, 99, 116, 105, 118, 97, 116, 101, 100, 18, 1, 49, 24, 1,
            ]),
            Some(5000),
        )
        .returning(Ok(first_call_token_id))
        .expect_log(
            Some(LogLevel::Debug),
            Some(format!("#2 initiated gRPC call (id# {first_call_token_id})").as_str()),
        )
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    let grpc_response: [u8; 2] = [8, 1];
    let second_call_token_id = first_call_token_id + 1;
    module
        .call_proxy_on_grpc_receive(
            http_context,
            first_call_token_id as i32,
            grpc_response.len() as i32,
        )
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 on_grpc_call_response: received gRPC call response: token: 42, status: 0"),
        )
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(&grpc_response))
        .expect_grpc_call(
            Some("unreachable-cluster"),
            Some("envoy.service.ratelimit.v3.RateLimitService"),
            Some("ShouldRateLimit"),
            Some(&[0, 0, 0, 0]),
            Some(&[
                10, 1, 97, 18, 28, 10, 26, 10, 21, 108, 105, 109, 105, 116, 95, 116, 111, 95, 98,
                101, 95, 97, 99, 116, 105, 118, 97, 116, 101, 100, 18, 1, 49, 24, 1,
            ]),
            Some(5000),
        )
        .returning(Ok(second_call_token_id))
        .execute_and_expect(ReturnType::None)
        .unwrap();

    let status_code = 14;
    module
        .proxy_on_grpc_close(http_context, second_call_token_id as i32, status_code)
        .expect_log(
            Some(LogLevel::Debug),
            Some(format!(
                "#2 on_grpc_call_response: received gRPC call response: token: {second_call_token_id}, status: {status_code}"
            ).as_str()),
        )
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(&[]))
        .expect_send_local_response(
            Some(500),
            Some("Internal Server Error.\n"),
            Some(vec![]),
            Some(-1),
        )
        .execute_and_expect(ReturnType::None)
        .unwrap();
}
