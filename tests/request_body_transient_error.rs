use crate::util::common::wasm_module;
use crate::util::data;
use proxy_wasm_test_framework::tester;
use proxy_wasm_test_framework::types::{Action, BufferType, LogLevel, MapType, ReturnType};
use serial_test::serial;

pub mod util;

#[test]
#[serial]
fn it_handles_transient_errors_for_request_body_json() {
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
    // Configuration with one action that requires requestBodyJSON evaluation
    // The requestData section contains CEL expressions that should return transient errors
    // So, the evaluation should be postponed until request body is available
    // Additionally, the requestData contains request attributes that should be
    // pre-fetched to be evaluated at the request body stage
    let cfg = r#"{
        "requestData": {
            "metrics.labels.model": "requestBodyJSON('/model')",
            "metrics.labels.max_tokens": "requestBodyJSON('/max_tokens')",
            "metrics.labels.scheme": "request.scheme"
        },
        "services": {
            "limitador": {
                "type": "ratelimit",
                "endpoint": "limitador-cluster",
                "failureMode": "deny",
                "timeout": "5s"
            }
        },
        "actionSets": [
        {
            "name": "test-transient-errors",
            "routeRuleConditions": {
                "hostnames": ["*.example.com"]
            },
            "actions": [
            {
                "service": "limitador",
                "scope": "test-scope",
                "conditionalData": [
                {
                    "predicates": [
                        "request.method == 'POST'"
                    ],
                    "data": [
                        {
                            "expression": {
                                "key": "static_key",
                                "value": "'static_value'"
                            }
                        },
                        {
                            "expression": {
                                "key": "body_model",
                                "value": "requestBodyJSON('/model')"
                            }
                        }
                    ]
                }]
            }
            ]
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

    // Step 1: on_http_request_headers should trigger transient error and set up AwaitRequestBody
    module
        .call_proxy_on_request_headers(http_context, 0, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_request_headers"))
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some(":authority"))
        .returning(Some("api.example.com"))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 action_set selected test-transient-errors"),
        )
        // retrieving tracing headers
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("traceparent"))
        .returning(None)
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("tracestate"))
        .returning(None)
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("baggage"))
        .returning(None)
        // This should trigger the condition evaluation
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"method\"]"),
        )
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(data::request::method::POST))
        // Expect the log when building descriptor starts
        .expect_log(
            Some(LogLevel::Debug),
            Some("build_descriptor(rl): starting, conditional_data_sets: 1"),
        )
        // Expect the log when waiting for request body due to transient evaluation errors
        .expect_log(
            Some(LogLevel::Info),
            Some("waiting for request body to be available"),
        )
        // The action should continue, indicating it's waiting for request body
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // Step 2: First call to on_http_request_body with partial body (end_of_stream=false)
    module
        .call_proxy_on_request_body(http_context, 50, false)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 on_http_request_body: body_size: 50, end_of_stream: false"),
        )
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    // Step 3: Second call to on_http_request_body with complete body (end_of_stream=true)
    let body = r#"{
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "Tell me about WASM filters"}],
        "max_tokens": 500,
        "temperature": 0.8
    }"#
    .as_bytes();

    module
        .call_proxy_on_request_body(http_context, body.len() as i32, true)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 on_http_request_body: body_size: 169, end_of_stream: true"),
        )
        .expect_get_buffer_bytes(Some(BufferType::HttpRequestBody))
        .returning(Some(body))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 on_http_request_body (size: 169): action_set selected test-transient-errors")
        )
        // Expect the build_descriptor log when processing with complete body
        .expect_log(
            Some(LogLevel::Debug),
            Some("build_descriptor(rl): starting, conditional_data_sets: 1"),
        )
        // Now the requestBodyJSON evaluation should succeed and grpc call should be made
        .expect_log(Some(LogLevel::Debug), Some("#2 send_grpc_request: limitador-cluster envoy.service.ratelimit.v3.RateLimitService ShouldRateLimit 5s"))
        .expect_grpc_call(
            Some("limitador-cluster"),
            Some("envoy.service.ratelimit.v3.RateLimitService"),
            Some("ShouldRateLimit"),
            Some(&[0, 0, 0, 0]),
            None,
            Some(5000),
        )
        .returning(Ok(42))
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    // Step 4: gRPC response handling
    let grpc_response: [u8; 2] = [8, 1]; // OK response
    module
        .call_proxy_on_grpc_receive(http_context, 42, grpc_response.len() as i32)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 on_grpc_call_response: received gRPC call response: token: 42, status: 0"),
        )
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(&grpc_response))
        .expect_log(
            Some(LogLevel::Debug),
            Some("process_response(rl): received OK response"),
        )
        .execute_and_expect(ReturnType::None)
        .unwrap();
}

#[test]
#[serial]
fn it_handles_multiple_transient_expressions_in_request_data() {
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
    // Configuration with request_data that mixes transient (requestBodyJSON) and
    // non-transient (request.method) expressions - this tests that the action waits
    // for transient data while non-transient data can be evaluated immediately
    let cfg = r#"{
        "requestData": {
            "metrics.labels.model": "requestBodyJSON('/model')",
            "metrics.labels.max_tokens": "requestBodyJSON('/max_tokens')",
            "metrics.labels.method": "request.method"
        },
        "services": {
            "telemetry": {
                "type": "ratelimit-report",
                "endpoint": "telemetry-cluster",
                "failureMode": "allow",
                "timeout": "1s"
            }
        },
        "actionSets": [
        {
            "name": "test-request-data-transient",
            "routeRuleConditions": {
                "hostnames": ["*.example.com"]
            },
            "actions": [
            {
                "service": "telemetry",
                "scope": "telemetry-scope",
                "conditionalData": [
                {
                    "predicates": [
                        "request.method == 'POST'"
                    ],
                    "data": [
                        {
                            "expression": {
                                "key": "static_key",
                                "value": "'static_value'"
                            }
                        },
                        {
                            "expression": {
                                "key": "body_model",
                                "value": "requestBodyJSON('/model')"
                            }
                        }
                    ]
                }]
            }
            ]
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

    // This should trigger transient error for requestData evaluation
    module
        .call_proxy_on_request_headers(http_context, 0, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_request_headers"))
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some(":authority"))
        .returning(Some("api.example.com"))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 action_set selected test-request-data-transient"),
        )
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("traceparent"))
        .returning(None)
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("tracestate"))
        .returning(None)
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("baggage"))
        .returning(None)
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"method\"]"),
        )
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(data::request::method::POST))
        // Expect the log when building descriptor starts
        .expect_log(
            Some(LogLevel::Debug),
            Some("build_descriptor(rl): starting, conditional_data_sets: 1"),
        )
        // Expect the log when waiting for request body due to transient evaluation errors
        .expect_log(
            Some(LogLevel::Info),
            Some("waiting for request body to be available"),
        )
        // The action should continue, indicating it's waiting for request body
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // Complete request body should allow evaluation to succeed
    let body = r#"{
        "model": "claude-3-sonnet",
        "messages": [{"role": "user", "content": "Hello"}],
        "max_tokens": 150,
        "temperature": 0.7
    }"#
    .as_bytes();

    module
        .call_proxy_on_request_body(http_context, body.len() as i32, true)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 on_http_request_body: body_size: 157, end_of_stream: true"),
        )
        .expect_get_buffer_bytes(Some(BufferType::HttpRequestBody))
        .returning(Some(body))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 on_http_request_body (size: 157): action_set selected test-request-data-transient")
        )
        // Expect the build_descriptor log when processing with complete body
        .expect_log(
            Some(LogLevel::Debug),
            Some("build_descriptor(rl): starting, conditional_data_sets: 1"),
        )
        // Now the request should succeed with the request_data containing model, max_tokens, and method
        .expect_log(Some(LogLevel::Debug), Some("#2 send_grpc_request: telemetry-cluster kuadrant.service.ratelimit.v1.RateLimitService Report 1s"))
        .expect_grpc_call(
            Some("telemetry-cluster"),
            Some("kuadrant.service.ratelimit.v1.RateLimitService"),
            Some("Report"),
            Some(&[0, 0, 0, 0]),
            None,
            Some(1000),
        )
        .returning(Ok(43))
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    // gRPC response handling
    let grpc_response: [u8; 2] = [8, 1];
    module
        .call_proxy_on_grpc_receive(http_context, 43, grpc_response.len() as i32)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 on_grpc_call_response: received gRPC call response: token: 43, status: 0"),
        )
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(&grpc_response))
        .expect_log(
            Some(LogLevel::Debug),
            Some("process_response(rl): received OK response"),
        )
        .execute_and_expect(ReturnType::None)
        .unwrap();
}
