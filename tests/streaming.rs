use crate::util::common::wasm_module;
use proxy_wasm_test_framework::tester;
use proxy_wasm_test_framework::types::{
    Action, BufferType, LogLevel, MapType, MetricType, ReturnType, Status,
};
use serial_test::serial;

pub mod util;

#[test]
#[serial]
fn it_processes_usage_event_across_chunks_until_done() {
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
    // One action requiring response body content via responseBodyJSON
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
                        "responseBodyJSON('/usage/total_tokens') == 11"
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
        .returning(Some("cars.toystore.com".as_bytes()))
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
        .failing_with(Status::BadArgument)
        // retrieving request attributes in advance
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `request.method`"),
        )
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(b"POST"))
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
        .returning(Some(vec![("content-type", "text/event-stream")]))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // First chunk: usage frame only (no DONE) → must not send gRPC yet
    let usage_chunk = b"data: {\"usage\":{\"total_tokens\":11}}\n\n";
    module
        .call_proxy_on_response_body(http_context, usage_chunk.len() as i32, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_response_body"))
        .expect_get_buffer_bytes(Some(BufferType::HttpResponseBody))
        .returning(Some(usage_chunk))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // Second chunk: DONE frame only → captures usage and sends gRPC on end_of_stream
    let done_chunk = b"data: [DONE]\n\n";
    let total_len = (usage_chunk.len() + done_chunk.len()) as i32;
    module
        .call_proxy_on_response_body(http_context, total_len, true)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_response_body"))
        .expect_get_buffer_bytes(Some(BufferType::HttpResponseBody))
        .returning(Some(done_chunk))
        // debug log about using generated id due to missing `x-request-id` header in request
        .expect_log(Some(LogLevel::Debug), None)
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
            Some("Getting gRPC response, size: 2 bytes"),
        )
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(&grpc_response))
        .execute_and_expect(ReturnType::None)
        .unwrap();
}

#[test]
#[serial]
fn it_streams_chunks_without_pausing_until_end_of_stream() {
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
                    "predicates": [
                        "responseBodyJSON('/usage/total_tokens') == 42"
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
        .returning(Some("cars.toystore.com".as_bytes()))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Selected blueprint some-name for hostname: cars.toystore.com"),
        )
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
        .failing_with(Status::BadArgument)
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `request.method`"),
        )
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(b"POST"))
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
        .returning(Some(vec![("content-type", "text/event-stream")]))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // Chunk 1: First message arrives (NOT DONE, no usage yet)
    let chunk1 = b"data: {\"id\":\"1\",\"content\":\"Hello\"}\n\n";
    module
        .call_proxy_on_response_body(http_context, chunk1.len() as i32, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_response_body"))
        .expect_get_buffer_bytes(Some(BufferType::HttpResponseBody))
        .returning(Some(chunk1))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // Chunk 2: Second message arrives (NOT DONE, no usage yet)
    let chunk2 = b"data: {\"id\":\"2\",\"content\":\"World\"}\n\n";
    module
        .call_proxy_on_response_body(http_context, chunk2.len() as i32, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_response_body"))
        .expect_get_buffer_bytes(Some(BufferType::HttpResponseBody))
        .returning(Some(chunk2))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // Chunk 3: Usage frame arrives (NOT DONE yet)
    let chunk3 = b"data: {\"usage\":{\"total_tokens\":42}}\n\n";
    module
        .call_proxy_on_response_body(http_context, chunk3.len() as i32, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_response_body"))
        .expect_get_buffer_bytes(Some(BufferType::HttpResponseBody))
        .returning(Some(chunk3))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // Chunk 4: DONE frame arrives (still NOT end_of_stream from Envoy's perspective)
    let chunk4 = b"data: [DONE]\n\n";
    module
        .call_proxy_on_response_body(http_context, chunk4.len() as i32, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_response_body"))
        .expect_get_buffer_bytes(Some(BufferType::HttpResponseBody))
        .returning(Some(chunk4))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // Finally: end_of_stream = true with 0 bytes (no new chunk) → NOW it should send the gRPC Report
    module
        .call_proxy_on_response_body(http_context, 0, true)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_response_body"))
        // debug log about using generated id due to missing `x-request-id` header in request
        .expect_log(Some(LogLevel::Debug), None)
        .expect_log(
            Some(LogLevel::Debug),
            Some("Dispatching gRPC call to limitador-cluster/kuadrant.service.ratelimit.v1.RateLimitService.Report, timeout: 5s"),
        )
        .expect_grpc_call(
            Some("limitador-cluster"),
            Some("kuadrant.service.ratelimit.v1.RateLimitService"),
            Some("Report"),
            None,
            None,
            Some(5000),
        )
        .returning(Ok(99))
        .expect_log(Some(LogLevel::Debug), Some("gRPC call dispatched successfully, token_id: 99"))
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    let grpc_response: [u8; 2] = [8, 1];
    module
        .call_proxy_on_grpc_receive(http_context, 99, grpc_response.len() as i32)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 on_grpc_call_response: received gRPC call response: token: 99, status: 0"),
        )
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting gRPC response, size: 2 bytes"),
        )
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(&grpc_response))
        .execute_and_expect(ReturnType::None)
        .unwrap();
}
