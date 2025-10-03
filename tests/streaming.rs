use crate::util::common::wasm_module;
use proxy_wasm_test_framework::tester;
use proxy_wasm_test_framework::types::{Action, BufferType, LogLevel, MapType, ReturnType};
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
        .returning(Some(b"POST"))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    module
        .call_proxy_on_response_headers(http_context, 0, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_response_headers"))
        .expect_get_header_map_value(Some(MapType::HttpResponseHeaders), Some("content-type"))
        .returning(Some("text/event-stream"))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 Content-Type: text/event-stream"),
        )
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // First chunk: usage frame only (no DONE) → must not send gRPC yet
    let usage_chunk = b"data: {\"usage\":{\"total_tokens\":11}}\n\n";
    module
        .call_proxy_on_response_body(http_context, usage_chunk.len() as i32, false)
        .expect_log(
            Some(LogLevel::Debug),
            Some(
                format!(
                    "#2 on_http_response_body: body_size: {}, end_of_stream: false",
                    usage_chunk.len()
                )
                .as_str(),
            ),
        )
        .expect_get_buffer_bytes(Some(BufferType::HttpResponseBody))
        .returning(Some(usage_chunk))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 handle_stream: processing chunk: data: {\"usage\":{\"total_tokens\":11}}\n\n"),
        )
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // Second chunk: DONE frame only → captures usage and sends gRPC on end_of_stream
    let done_chunk = b"data: [DONE]\n\n";
    let total_len = (usage_chunk.len() + done_chunk.len()) as i32;
    module
        .call_proxy_on_response_body(http_context, total_len, true)
        .expect_log(
            Some(LogLevel::Debug),
            Some(
                format!(
                    "#2 on_http_response_body: body_size: {}, end_of_stream: true",
                    total_len
                )
                .as_str(),
            ),
        )
        .expect_get_buffer_bytes(Some(BufferType::HttpResponseBody))
        .returning(Some(done_chunk))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 handle_stream: processing chunk: data: [DONE]\n\n"),
        )
        .expect_log(Some(LogLevel::Debug), Some("#2 send_grpc_request: limitador-cluster kuadrant.service.ratelimit.v1.RateLimitService Report 5s"))
        .expect_grpc_call(
            Some("limitador-cluster"),
            Some("kuadrant.service.ratelimit.v1.RateLimitService"),
            Some("Report"),
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
        .returning(Some(b"POST"))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    module
        .call_proxy_on_response_headers(http_context, 0, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_response_headers"))
        .expect_get_header_map_value(Some(MapType::HttpResponseHeaders), Some("content-type"))
        .returning(Some("text/event-stream"))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 Content-Type: text/event-stream"),
        )
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // Chunk 1: First message arrives (NOT DONE, no usage yet)
    let chunk1 = b"data: {\"id\":\"1\",\"content\":\"Hello\"}\n\n";
    module
        .call_proxy_on_response_body(http_context, chunk1.len() as i32, false)
        .expect_log(
            Some(LogLevel::Debug),
            Some(
                format!(
                    "#2 on_http_response_body: body_size: {}, end_of_stream: false",
                    chunk1.len()
                )
                .as_str(),
            ),
        )
        .expect_get_buffer_bytes(Some(BufferType::HttpResponseBody))
        .returning(Some(chunk1))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 handle_stream: processing chunk: data: {\"id\":\"1\",\"content\":\"Hello\"}\n\n"),
        )
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // Chunk 2: Second message arrives (NOT DONE, no usage yet)
    let chunk2 = b"data: {\"id\":\"2\",\"content\":\"World\"}\n\n";
    module
        .call_proxy_on_response_body(http_context, chunk2.len() as i32, false)
        .expect_log(
            Some(LogLevel::Debug),
            Some(
                format!(
                    "#2 on_http_response_body: body_size: {}, end_of_stream: false",
                    chunk2.len()
                )
                .as_str(),
            ),
        )
        .expect_get_buffer_bytes(Some(BufferType::HttpResponseBody))
        .returning(Some(chunk2))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 handle_stream: processing chunk: data: {\"id\":\"2\",\"content\":\"World\"}\n\n"),
        )
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // Chunk 3: Usage frame arrives (NOT DONE yet)
    let chunk3 = b"data: {\"usage\":{\"total_tokens\":42}}\n\n";
    module
        .call_proxy_on_response_body(http_context, chunk3.len() as i32, false)
        .expect_log(
            Some(LogLevel::Debug),
            Some(
                format!(
                    "#2 on_http_response_body: body_size: {}, end_of_stream: false",
                    chunk3.len()
                )
                .as_str(),
            ),
        )
        .expect_get_buffer_bytes(Some(BufferType::HttpResponseBody))
        .returning(Some(chunk3))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 handle_stream: processing chunk: data: {\"usage\":{\"total_tokens\":42}}\n\n"),
        )
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // Chunk 4: DONE frame arrives (still NOT end_of_stream from Envoy's perspective)
    let chunk4 = b"data: [DONE]\n\n";
    module
        .call_proxy_on_response_body(http_context, chunk4.len() as i32, false)
        .expect_log(
            Some(LogLevel::Debug),
            Some(
                format!(
                    "#2 on_http_response_body: body_size: {}, end_of_stream: false",
                    chunk4.len()
                )
                .as_str(),
            ),
        )
        .expect_get_buffer_bytes(Some(BufferType::HttpResponseBody))
        .returning(Some(chunk4))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 handle_stream: processing chunk: data: [DONE]\n\n"),
        )
        // Should NOT send gRPC yet - still waiting for end_of_stream
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // Finally: end_of_stream = true with 0 bytes (no new chunk) → NOW it should send the gRPC Report
    module
        .call_proxy_on_response_body(http_context, 0, true)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 on_http_response_body: body_size: 0, end_of_stream: true"),
        )
        .expect_log(Some(LogLevel::Debug), Some("#2 send_grpc_request: limitador-cluster kuadrant.service.ratelimit.v1.RateLimitService Report 5s"))
        .expect_grpc_call(
            Some("limitador-cluster"),
            Some("kuadrant.service.ratelimit.v1.RateLimitService"),
            Some("Report"),
            Some(&[0, 0, 0, 0]),
            None,
            Some(5000),
        )
        .returning(Ok(99))
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    let grpc_response: [u8; 2] = [8, 1];
    module
        .call_proxy_on_grpc_receive(http_context, 99, grpc_response.len() as i32)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 on_grpc_call_response: received gRPC call response: token: 99, status: 0"),
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
