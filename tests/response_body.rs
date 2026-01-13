use crate::util::common::wasm_module;
use crate::util::data;
use proxy_wasm_test_framework::tester;
use proxy_wasm_test_framework::types::{
    Action, BufferType, LogLevel, MapType, MetricType, ReturnType,
};
use serial_test::serial;

pub mod util;

#[test]
#[serial]
fn it_checks_and_reports() {
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
    // Two actions configured:
    // 1. check which runs in the request phase
    // 2. report which requires the request headers and the response body
    let cfg = r#"{
        "services": {
            "limitador-check": {
                "type": "ratelimit-check",
                "endpoint": "limitador-cluster",
                "failureMode": "deny",
                "timeout": "5s"
            },
            "limitador-report": {
                "type": "ratelimit-report",
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
                "service": "limitador-check",
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
                "service": "limitador-report",
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
        .expect_define_metric(Some(MetricType::Counter), Some("kuadrant.configs"))
        .returning(Some(1))
        .expect_define_metric(Some(MetricType::Counter), Some("kuadrant.hits"))
        .returning(Some(2))
        .expect_define_metric(Some(MetricType::Counter), Some("kuadrant.misses"))
        .returning(Some(3))
        .expect_define_metric(Some(MetricType::Counter), Some("kuadrant.allowed"))
        .returning(Some(4))
        .expect_define_metric(Some(MetricType::Counter), Some("kuadrant.denied"))
        .returning(Some(5))
        .expect_define_metric(Some(MetricType::Counter), Some("kuadrant.errors"))
        .returning(Some(6))
        .expect_increment_metric(Some(1), Some(1))
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
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `request.host`"),
        )
        .expect_get_property(Some(vec!["request", "host"]))
        .returning(Some(data::request::HOST))
        // retrieving properties for conditions
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `request.url_path`"),
        )
        .expect_get_property(Some(vec!["request", "url_path"]))
        .returning(Some(data::request::path::ADMIN_TOY))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Selected blueprint some-name for hostname: cars.toystore.com"),
        )
        // retrieving tracing headers
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting map: `HttpRequestHeaders`"),
        )
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(None)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 pipeline built successfully"),
        )
        .expect_increment_metric(Some(2), Some(1))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `request.method`"),
        )
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(data::request::method::POST))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Dispatching gRPC call to limitador-cluster/kuadrant.service.ratelimit.v1.RateLimitService.CheckRateLimit, timeout: 5s")
        )
        .expect_grpc_call(
            Some("limitador-cluster"),
            Some("kuadrant.service.ratelimit.v1.RateLimitService"),
            Some("CheckRateLimit"),
            None,
            None,
            Some(5000),
        )
        .returning(Ok(42))
        .expect_log(
            Some(LogLevel::Debug),
            Some("gRPC call dispatched successfully, token_id: 42"),
        )
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting map: `HttpResponseHeaders`"),
        )
        .expect_get_header_map_pairs(Some(MapType::HttpResponseHeaders))
        .failing_with(proxy_wasm_test_framework::types::Status::BadArgument)
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    let grpc_response: [u8; 2] = [8, 1];
    module
        .call_proxy_on_grpc_receive(http_context, 42, grpc_response.len() as i32)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 on_grpc_call_response: received gRPC call response: token: 42, status: 0"),
        )
        .expect_log(
            Some(LogLevel::Debug),
            Some(format!("Getting gRPC response, size: {} bytes", grpc_response.len()).as_str()),
        )
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(&grpc_response))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting map: `HttpResponseHeaders`"),
        )
        .expect_get_header_map_pairs(Some(MapType::HttpResponseHeaders))
        .failing_with(proxy_wasm_test_framework::types::Status::BadArgument)
        .execute_and_expect(ReturnType::None)
        .unwrap();

    module
        .call_proxy_on_request_body(http_context, 25i32, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_request_body"))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting map: `HttpResponseHeaders`"),
        )
        .expect_get_header_map_pairs(Some(MapType::HttpResponseHeaders))
        .failing_with(proxy_wasm_test_framework::types::Status::BadArgument)
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    module
        .call_proxy_on_response_headers(http_context, 0, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_response_headers"))
        .expect_increment_metric(Some(4), Some(1))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting map: `HttpResponseHeaders`"),
        )
        .expect_get_header_map_pairs(Some(MapType::HttpResponseHeaders))
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
        .call_proxy_on_response_body(http_context, response_body.len() as i32, true)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_response_body"))
        .expect_get_buffer_bytes(Some(BufferType::HttpResponseBody))
        .returning(Some(response_body))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Dispatching gRPC call to limitador-cluster/kuadrant.service.ratelimit.v1.RateLimitService.Report, timeout: 5s")
        )
        .expect_grpc_call(
            Some("limitador-cluster"),
            Some("kuadrant.service.ratelimit.v1.RateLimitService"),
            Some("Report"),
            None,
            None,
            Some(5000),
        )
        .returning(Ok(43))
        .expect_log(
            Some(LogLevel::Debug),
            Some("gRPC call dispatched successfully, token_id: 43"),
        )
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    module
        .call_proxy_on_grpc_receive(http_context, 43, grpc_response.len() as i32)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 on_grpc_call_response: received gRPC call response: token: 43, status: 0"),
        )
        .expect_log(
            Some(LogLevel::Debug),
            Some(format!("Getting gRPC response, size: {} bytes", grpc_response.len()).as_str()),
        )
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(&grpc_response))
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
                "type": "ratelimit-report",
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
        .expect_define_metric(Some(MetricType::Counter), Some("kuadrant.configs"))
        .returning(Some(1))
        .expect_define_metric(Some(MetricType::Counter), Some("kuadrant.hits"))
        .returning(Some(2))
        .expect_define_metric(Some(MetricType::Counter), Some("kuadrant.misses"))
        .returning(Some(3))
        .expect_define_metric(Some(MetricType::Counter), Some("kuadrant.allowed"))
        .returning(Some(4))
        .expect_define_metric(Some(MetricType::Counter), Some("kuadrant.denied"))
        .returning(Some(5))
        .expect_define_metric(Some(MetricType::Counter), Some("kuadrant.errors"))
        .returning(Some(6))
        .expect_increment_metric(Some(1), Some(1))
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
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `request.host`"),
        )
        .expect_get_property(Some(vec!["request", "host"]))
        .returning(Some(data::request::HOST))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Selected blueprint some-name for hostname: cars.toystore.com"),
        )
        // retrieving tracing headers
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting map: `HttpRequestHeaders`"),
        )
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(None)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 pipeline built successfully"),
        )
        .expect_increment_metric(Some(2), Some(1))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting map: `HttpResponseHeaders`"),
        )
        .expect_get_header_map_pairs(Some(MapType::HttpResponseHeaders))
        .failing_with(proxy_wasm_test_framework::types::Status::BadArgument)
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `request.url_path`"),
        )
        .expect_get_property(Some(vec!["request", "url_path"]))
        .returning(Some(data::request::path::ADMIN_TOY))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `request.method`"),
        )
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(data::request::method::POST))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    module
        .call_proxy_on_request_body(http_context, 25i32, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_request_body"))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting map: `HttpResponseHeaders`"),
        )
        .expect_get_header_map_pairs(Some(MapType::HttpResponseHeaders))
        .failing_with(proxy_wasm_test_framework::types::Status::BadArgument)
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    module
        .call_proxy_on_response_headers(http_context, 0, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_response_headers"))
        .expect_increment_metric(Some(4), Some(1))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting map: `HttpResponseHeaders`"),
        )
        .expect_get_header_map_pairs(Some(MapType::HttpResponseHeaders))
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
        .call_proxy_on_response_body(http_context, response_body.len() as i32, true)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_response_body"))
        .expect_get_buffer_bytes(Some(BufferType::HttpResponseBody))
        .returning(Some(response_body))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Dispatching gRPC call to limitador-cluster/kuadrant.service.ratelimit.v1.RateLimitService.Report, timeout: 5s")
        )
        .expect_grpc_call(
            Some("limitador-cluster"),
            Some("kuadrant.service.ratelimit.v1.RateLimitService"),
            Some("Report"),
            None,
            None,
            Some(5000),
        )
        .returning(Ok(42))
        .expect_log(Some(LogLevel::Debug), Some("gRPC call dispatched successfully, token_id: 42"))
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    let grpc_response: [u8; 2] = [8, 1];
    module
        .call_proxy_on_grpc_receive(http_context, 42, grpc_response.len() as i32)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 on_grpc_call_response: received gRPC call response: token: 42, status: 0"),
        )
        .expect_log(
            Some(LogLevel::Debug),
            Some(format!("Getting gRPC response, size: {} bytes", grpc_response.len()).as_str()),
        )
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(&grpc_response))
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
                "type": "ratelimit-report",
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
        .expect_define_metric(Some(MetricType::Counter), Some("kuadrant.configs"))
        .returning(Some(1))
        .expect_define_metric(Some(MetricType::Counter), Some("kuadrant.hits"))
        .returning(Some(2))
        .expect_define_metric(Some(MetricType::Counter), Some("kuadrant.misses"))
        .returning(Some(3))
        .expect_define_metric(Some(MetricType::Counter), Some("kuadrant.allowed"))
        .returning(Some(4))
        .expect_define_metric(Some(MetricType::Counter), Some("kuadrant.denied"))
        .returning(Some(5))
        .expect_define_metric(Some(MetricType::Counter), Some("kuadrant.errors"))
        .returning(Some(6))
        .expect_increment_metric(Some(1), Some(1))
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
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `request.host`"),
        )
        .expect_get_property(Some(vec!["request", "host"]))
        .returning(Some(data::request::HOST))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Selected blueprint some-name for hostname: cars.toystore.com"),
        )
        // retrieving tracing headers
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting map: `HttpRequestHeaders`"),
        )
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(None)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 pipeline built successfully"),
        )
        .expect_increment_metric(Some(2), Some(1))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting map: `HttpResponseHeaders`"),
        )
        .expect_get_header_map_pairs(Some(MapType::HttpResponseHeaders))
        .failing_with(proxy_wasm_test_framework::types::Status::BadArgument)
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    module
        .call_proxy_on_request_body(http_context, 25i32, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_request_body"))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting map: `HttpResponseHeaders`"),
        )
        .expect_get_header_map_pairs(Some(MapType::HttpResponseHeaders))
        .failing_with(proxy_wasm_test_framework::types::Status::BadArgument)
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    module
        .call_proxy_on_response_headers(http_context, 0, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_response_headers"))
        .expect_increment_metric(Some(4), Some(1))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting map: `HttpResponseHeaders`"),
        )
        .expect_get_header_map_pairs(Some(MapType::HttpResponseHeaders))
        .returning(None)
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    let response_body = "some crap that cannot be JSON parsed".as_bytes();
    module
        .call_proxy_on_response_body(http_context, response_body.len() as i32, true)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_response_body"))
        .expect_get_buffer_bytes(Some(BufferType::HttpResponseBody))
        .returning(Some(response_body))
        .expect_log(
            Some(LogLevel::Warn),
            Some("Missing json property: /usage/total_tokens"),
        )
        // on response headers/body, expected action is Continue
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();
}
