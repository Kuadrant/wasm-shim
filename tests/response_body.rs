use crate::util::common::wasm_module;
use crate::util::data;
use proxy_wasm_test_framework::tester;
use proxy_wasm_test_framework::types::{Action, BufferType, LogLevel, MapType, ReturnType};
use serial_test::serial;

pub mod util;

#[test]
#[serial]
fn it_waits_for_the_response_body() {
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
    // Three actions configured:
    // 1. only requires the request headers
    // 2. requires the request headers and the response body
    // 3. only requires the request headers which has not been read yet (request.host)
    let cfg = r#"{
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
            "name": "some-name",
            "routeRuleConditions": {
                "hostnames": ["*.toystore.com", "example.com"],
                "predicates" : [
                    "request.url_path.startsWith('/admin/toy')"
                ]
            },
            "actions": [
            {
                "service": "limitador",
                "scope": "RLS-domain-A",
                "conditionalData": [
                {
                    "predicates": [
                        "request.method == 'POST'"
                    ],
                    "data": [
                        {
                            "expression": {
                                "key": "request.method",
                                "value": "request.method"
                            }
                        }
                    ]
                }]
            },
            {
                "service": "limitador",
                "scope": "RLS-domain-B",
                "conditionalData": [
                {
                    "predicates": [
                        "request.method == 'POST'"
                    ],
                    "data": [
                        {
                            "expression": {
                                "key": "model",
                                "value": "responseBodyJSON('/usage/total_tokens')"
                            }
                        }
                    ]
                }]
            },
            {
                "service": "limitador",
                "scope": "RLS-domain-C",
                "conditionalData": [
                {
                    "predicates": [
                        "request.method == 'POST'"
                    ],
                    "data": [
                        {
                            "expression": {
                                "key": "request.host",
                                "value": "request.host == 'cars.toystore.com'"
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

    module
        .call_proxy_on_request_headers(http_context, 0, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_request_headers"))
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some(":authority"))
        .returning(Some("cars.toystore.com"))
        // retrieving properties for conditions
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"url_path\"]"),
        )
        .expect_get_property(Some(vec!["request", "url_path"]))
        .returning(Some(data::request::path::ADMIN_TOY))
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
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"method\"]"),
        )
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(data::request::method::POST))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"host\"]"),
        )
        .expect_get_property(Some(vec!["request", "host"]))
        .returning(Some(data::request::HOST))
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

    let grpc_response: [u8; 2] = [8, 1];
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

    module
        .call_proxy_on_request_body(http_context, 25i32, false)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 on_http_request_body: body_size: 25, end_of_stream: false"),
        )
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    module
        .call_proxy_on_response_headers(http_context, 0, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_response_headers"))
        .expect_get_header_map_value(Some(MapType::HttpResponseHeaders), Some("content-type"))
        .returning(None)
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    let response_body = r#"
        {
          "id": "chatcmpl-2ee7427f-8b51-4f74-a5df-e6484df42547",
          "model": "meta-llama/Llama-3.1-8B-Instruct",
          "usage": {
            "prompt_tokens": 0,
            "completion_tokens": 11,
            "total_tokens": 11
          },
          "choices": []
        }"#
    .as_bytes();
    module
        .call_proxy_on_response_body(http_context, 25i32, false)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 on_http_response_body: body_size: 25, end_of_stream: false"),
        )
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    module
        .call_proxy_on_response_body(http_context, response_body.len() as i32, true)
        .expect_log(
            Some(LogLevel::Debug),
            Some(format!(
                    "#2 on_http_response_body: body_size: {}, end_of_stream: true", 
                    response_body.len()
                    ).as_str()
                ),
        )
        .expect_get_buffer_bytes(Some(BufferType::HttpResponseBody))
        .returning(Some(response_body))
        .expect_log(
            Some(LogLevel::Debug),
            Some(format!(
                    "#2 on_http_response_body (size: {}): action_set selected some-name",
                    response_body.len()
                    ).as_str()
                ),
        )
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
        .execute_and_expect(ReturnType::None)
        .unwrap();

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
fn it_reads_request_attr_in_advance_when_response_body() {
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
    // There is only one action that requires response body on predicates
    // That action also has predicates and data expressions that require request attributes
    // tracing headers also need to be read in advance
    let cfg = r#"{
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
            "name": "some-name",
            "routeRuleConditions": {
                "hostnames": ["*.toystore.com", "example.com"]
            },
            "actions": [
            {
                "service": "limitador",
                "scope": "RLS-domain",
                "conditionalData": [
                {
                    "predicates": [
                        "responseBodyJSON('/usage/total_tokens') == 11",
                        "request.url_path.startsWith('/admin/toy')"
                    ],
                    "data": [
                        {
                            "expression": {
                                "key": "request.method",
                                "value": "request.method"
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

    module
        .call_proxy_on_request_headers(http_context, 0, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_request_headers"))
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some(":authority"))
        .returning(Some("cars.toystore.com"))
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
        // retrieving request attributes in advance
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"method\"]"),
        )
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(data::request::method::POST))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"url_path\"]"),
        )
        .expect_get_property(Some(vec!["request", "url_path"]))
        .returning(Some(data::request::path::ADMIN_TOY))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    module
        .call_proxy_on_request_body(http_context, 25i32, false)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 on_http_request_body: body_size: 25, end_of_stream: false"),
        )
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    module
        .call_proxy_on_response_headers(http_context, 0, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_response_headers"))
        .expect_get_header_map_value(Some(MapType::HttpResponseHeaders), Some("content-type"))
        .returning(None)
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    let response_body = r#"
        {
          "id": "chatcmpl-2ee7427f-8b51-4f74-a5df-e6484df42547",
          "model": "meta-llama/Llama-3.1-8B-Instruct",
          "usage": {
            "prompt_tokens": 0,
            "completion_tokens": 11,
            "total_tokens": 11
          },
          "choices": []
        }"#
    .as_bytes();
    module
        .call_proxy_on_response_body(http_context, 25i32, false)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 on_http_response_body: body_size: 25, end_of_stream: false"),
        )
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    module
        .call_proxy_on_response_body(http_context, response_body.len() as i32, true)
        .expect_log(
            Some(LogLevel::Debug),
            Some(format!(
                    "#2 on_http_response_body: body_size: {}, end_of_stream: true", 
                    response_body.len()
                    ).as_str()
                ),
        )
        .expect_get_buffer_bytes(Some(BufferType::HttpResponseBody))
        .returning(Some(response_body))
        .expect_log(
            Some(LogLevel::Debug),
            Some(format!(
                    "#2 on_http_response_body (size: {}): action_set selected some-name",
                    response_body.len()
                    ).as_str()
                ),
        )
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

    let grpc_response: [u8; 2] = [8, 1];
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
fn it_calls_action_with_request_and_response_body() {
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
            }
        },
        "actionSets": [
        {
            "name": "some-name",
            "routeRuleConditions": {
                "hostnames": ["*.toystore.com", "example.com"],
                "predicates" : [
                    "request.url_path.startsWith('/admin/toy')"
                ]
            },
            "actions": [
            {
                "service": "limitador",
                "scope": "RLS-domain-B",
                "conditionalData": [
                {
                    "predicates": [
                        "request.method == 'POST'"
                    ],
                    "data": [
                        {
                            "expression": {
                                "key": "model",
                                "value": "requestBodyJSON('/model')"
                            }
                        },
                        {
                            "expression": {
                                "key": "ratelimit.hits_addend",
                                "value": "responseBodyJSON('/usage/total_tokens')"
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

    module
        .call_proxy_on_request_headers(http_context, 0, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_request_headers"))
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some(":authority"))
        .returning(Some("cars.toystore.com"))
        // retrieving properties for conditions
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"url_path\"]"),
        )
        .expect_get_property(Some(vec!["request", "url_path"]))
        .returning(Some(data::request::path::ADMIN_TOY))
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
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"method\"]"),
        )
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(data::request::method::POST))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    module
        .call_proxy_on_request_body(http_context, 25i32, false)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 on_http_request_body: body_size: 25, end_of_stream: false"),
        )
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    let request_body = r#"
        {
            "model": "meta-llama/Llama-3.1-8B-Instruct",
            "input": "Tell me a three sentence story about a robot."
        }"#
    .as_bytes();
    module
        .call_proxy_on_request_body(http_context, request_body.len() as i32, true)
        .expect_log(
            Some(LogLevel::Debug),
            Some(
                format!(
                    "#2 on_http_request_body: body_size: {}, end_of_stream: true",
                    request_body.len()
                )
                .as_str(),
            ),
        )
        .expect_get_buffer_bytes(Some(BufferType::HttpRequestBody))
        .returning(Some(request_body))
        .expect_log(
            Some(LogLevel::Debug),
            Some(
                format!(
                    "#2 on_http_request_body (size: {}): action_set selected some-name",
                    request_body.len(),
                )
                .as_str(),
            ),
        )
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    module
        .call_proxy_on_response_headers(http_context, 0, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_response_headers"))
        .expect_get_header_map_value(Some(MapType::HttpResponseHeaders), Some("content-type"))
        .returning(None)
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    let response_body = r#"
        {
          "id": "chatcmpl-2ee7427f-8b51-4f74-a5df-e6484df42547",
          "model": "meta-llama/Llama-3.1-8B-Instruct",
          "usage": {
            "prompt_tokens": 0,
            "completion_tokens": 11,
            "total_tokens": 11
          },
          "choices": []
        }"#
    .as_bytes();
    module
        .call_proxy_on_response_body(http_context, 25i32, false)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 on_http_response_body: body_size: 25, end_of_stream: false"),
        )
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    module
        .call_proxy_on_response_body(http_context, response_body.len() as i32, true)
        .expect_log(
            Some(LogLevel::Debug),
            Some(format!(
                    "#2 on_http_response_body: body_size: {}, end_of_stream: true", 
                    response_body.len()
                    ).as_str()
                ),
        )
        .expect_get_buffer_bytes(Some(BufferType::HttpResponseBody))
        .returning(Some(response_body))
        .expect_log(
            Some(LogLevel::Debug),
            Some(format!(
                    "#2 on_http_response_body (size: {}): action_set selected some-name",
                    response_body.len()
                    ).as_str()
                ),
        )
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

    let grpc_response: [u8; 2] = [8, 1];
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
fn it_handles_errors_on_response_body() {
    // The error will be body that cannot be parsed as JSON object
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
            }
        },
        "actionSets": [
        {
            "name": "some-name",
            "routeRuleConditions": {
                "hostnames": ["*.toystore.com", "example.com"]
            },
            "actions": [
            {
                "service": "limitador",
                "scope": "RLS-domain",
                "conditionalData": [
                {
                    "data": [
                        {
                            "expression": {
                                "key": "a",
                                "value": "'1'"
                            }
                        },
                        {
                            "expression": {
                                "key": "ratelimit.hits_addend",
                                "value": "responseBodyJSON('/usage/total_tokens')"
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

    module
        .call_proxy_on_request_headers(http_context, 0, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_request_headers"))
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some(":authority"))
        .returning(Some("cars.toystore.com"))
        // retrieving properties for conditions
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
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    module
        .call_proxy_on_request_body(http_context, 25i32, false)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 on_http_request_body: body_size: 25, end_of_stream: false"),
        )
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    module
        .call_proxy_on_response_headers(http_context, 0, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_response_headers"))
        .expect_get_header_map_value(Some(MapType::HttpResponseHeaders), Some("content-type"))
        .returning(None)
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    let response_body = "some crap that cannot be JSON parsed".as_bytes();
    module
        .call_proxy_on_response_body(http_context, 25i32, false)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 on_http_response_body: body_size: 25, end_of_stream: false"),
        )
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    module
        .call_proxy_on_response_body(http_context, response_body.len() as i32, true)
        .expect_log(
            Some(LogLevel::Debug),
            Some(
                format!(
                    "#2 on_http_response_body: body_size: {}, end_of_stream: true",
                    response_body.len()
                )
                .as_str(),
            ),
        )
        .expect_get_buffer_bytes(Some(BufferType::HttpResponseBody))
        .returning(Some(response_body))
        .expect_log(
            Some(LogLevel::Debug),
            Some(
                format!(
                    "#2 on_http_response_body (size: {}): action_set selected some-name",
                    response_body.len()
                )
                .as_str(),
            ),
        )
        .expect_log(Some(LogLevel::Debug), None)
        .expect_log(
            Some(LogLevel::Debug),
            Some("Ignoring trying to send direct response after phase has ended!"),
        )
        // on response headers/body, expected action is Continue
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();
}
