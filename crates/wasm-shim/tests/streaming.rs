use crate::util::common::{json_escape_cel, wasm_module, LOG_LEVEL};
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
    let report_msg = r#"
        envoy.service.ratelimit.v3.RateLimitRequest {
            domain: "RLS-domain",
            hits_addend: 1u,
            descriptors: [
                envoy.extensions.common.ratelimit.v3.RateLimitDescriptor {
                    entries: (
                        (kuadrant.internal.response.body.total_tokens == 11) ?
                        [envoy.extensions.common.ratelimit.v3.RateLimitDescriptor.Entry {
                            key: "request.method",
                            value: string(request.method)
                        }] :
                        []
                    )
                }
            ]
        }
    "#;
    let cfg = r#"{
        "services": {
            "limitador": {
                "type": "dynamic",
                "endpoint": "limitador-cluster",
                "failureMode": "deny",
                "timeout": "5s",
                "grpcService": "kuadrant.service.ratelimit.v1.RateLimitService",
                "grpcMethod": "Report"
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
                "type": "store",
                "predicate": "true",
                "terminal": false,
                "path": "kuadrant.internal.response.body",
                "value": "{\"total_tokens\": responseBodyJSON('/usage/total_tokens')}"
            },
            {
                "type": "grpc",
                "var": "report_response",
                "service": "limitador",
                "predicate": "kuadrant.internal.response.body.total_tokens == 11",
                "terminal": false,
                "isGuard": false,
                "label": "ratelimit_report",
                "messageBuilder": "__REPORT_MSG__",
                "onReply": [
                    {
                        "type": "fail",
                        "predicate": "!has(report_response.overall_code)",
                        "terminal": false,
                        "isGuard": false,
                        "logMessage": "Rate limit report failed: invalid gRPC response"
                    }
                ]
            }
            ]
        }]
    }"#
    .replace("__REPORT_MSG__", &json_escape_cel(report_msg));

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
        .expect_define_metric(
            Some(MetricType::Counter),
            Some("kuadrant.body_extraction_misses"),
        )
        .returning(Some(7))
        .expect_increment_metric(Some(1), Some(1))
        .expect_get_buffer_bytes(Some(BufferType::PluginConfiguration))
        .returning(Some(cfg.as_bytes()))
        .expect_get_log_level()
        .returning(Some(LOG_LEVEL))
        .execute_and_expect(ReturnType::Bool(true))
        .unwrap();

    let http_context = 2;
    module
        .call_proxy_on_context_create(http_context, root_context)
        .expect_get_log_level()
        .returning(Some(LOG_LEVEL))
        .execute_and_expect(ReturnType::None)
        .unwrap();

    module
        .call_proxy_on_request_headers(http_context, 0, false)
        .expect_get_property(Some(vec!["request", "host"]))
        .returning(Some("cars.toystore.com".as_bytes()))
        // retrieving tracing headers
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(None)
        .expect_increment_metric(Some(2), Some(1))
        .expect_get_header_map_pairs(Some(MapType::HttpResponseHeaders))
        .failing_with(Status::BadArgument)
        // retrieving request attributes in advance
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(b"POST"))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    module
        .call_proxy_on_response_headers(http_context, 0, false)
        .expect_increment_metric(Some(4), Some(1))
        .expect_get_header_map_pairs(Some(MapType::HttpResponseHeaders))
        .returning(Some(vec![("content-type", "text/event-stream")]))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // First chunk: usage frame arrives → the SSE parser resolves the field
    // incrementally as soon as it's seen, so the gRPC report is dispatched
    // right away rather than waiting for end_of_stream. The report action is
    // not a guard, so the filter chain still continues immediately.
    let usage_chunk = b"data: {\"usage\":{\"total_tokens\":11}}\n\n";
    module
        .call_proxy_on_response_body(http_context, usage_chunk.len() as i32, false)
        .expect_get_buffer_bytes(Some(BufferType::HttpResponseBody))
        .returning(Some(usage_chunk))
        .expect_grpc_call(
            Some("limitador-cluster"),
            Some("kuadrant.service.ratelimit.v1.RateLimitService"),
            Some("Report"),
            None,
            None,
            Some(5000),
        )
        .returning(Ok(42))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // Second chunk: DONE frame arrives at end_of_stream, with the gRPC report
    // still in flight → the filter must pause until the response is digested.
    let done_chunk = b"data: [DONE]\n\n";
    module
        .call_proxy_on_response_body(http_context, done_chunk.len() as i32, true)
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    let grpc_response: [u8; 2] = [8, 1];
    module
        .call_proxy_on_grpc_receive(http_context, 42, grpc_response.len() as i32)
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
    let report_msg = r#"
        envoy.service.ratelimit.v3.RateLimitRequest {
            domain: "RLS-domain",
            hits_addend: 1u,
            descriptors: [
                envoy.extensions.common.ratelimit.v3.RateLimitDescriptor {
                    entries: (
                        (kuadrant.internal.response.body.total_tokens == 42) ?
                        [envoy.extensions.common.ratelimit.v3.RateLimitDescriptor.Entry {
                            key: "request.method",
                            value: string(request.method)
                        }] :
                        []
                    )
                }
            ]
        }
    "#;
    let cfg = r#"{
        "services": {
            "limitador": {
                "type": "dynamic",
                "endpoint": "limitador-cluster",
                "failureMode": "deny",
                "timeout": "5s",
                "grpcService": "kuadrant.service.ratelimit.v1.RateLimitService",
                "grpcMethod": "Report"
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
                "type": "store",
                "predicate": "true",
                "terminal": false,
                "path": "kuadrant.internal.response.body",
                "value": "{\"total_tokens\": responseBodyJSON('/usage/total_tokens')}"
            },
            {
                "type": "grpc",
                "var": "report_response",
                "service": "limitador",
                "predicate": "kuadrant.internal.response.body.total_tokens == 42",
                "terminal": false,
                "isGuard": false,
                "label": "ratelimit_report",
                "messageBuilder": "__REPORT_MSG__",
                "onReply": [
                    {
                        "type": "fail",
                        "predicate": "!has(report_response.overall_code)",
                        "terminal": false,
                        "isGuard": false,
                        "logMessage": "Rate limit report failed: invalid gRPC response"
                    }
                ]
            }
            ]
        }]
    }"#
    .replace("__REPORT_MSG__", &json_escape_cel(report_msg));

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
        .expect_define_metric(
            Some(MetricType::Counter),
            Some("kuadrant.body_extraction_misses"),
        )
        .returning(Some(7))
        .expect_increment_metric(Some(1), Some(1))
        .expect_get_buffer_bytes(Some(BufferType::PluginConfiguration))
        .returning(Some(cfg.as_bytes()))
        .expect_get_log_level()
        .returning(Some(LOG_LEVEL))
        .execute_and_expect(ReturnType::Bool(true))
        .unwrap();

    let http_context = 2;
    module
        .call_proxy_on_context_create(http_context, root_context)
        .expect_get_log_level()
        .returning(Some(LOG_LEVEL))
        .execute_and_expect(ReturnType::None)
        .unwrap();

    module
        .call_proxy_on_request_headers(http_context, 0, false)
        .expect_get_property(Some(vec!["request", "host"]))
        .returning(Some("cars.toystore.com".as_bytes()))
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(None)
        .expect_increment_metric(Some(2), Some(1))
        .expect_get_header_map_pairs(Some(MapType::HttpResponseHeaders))
        .failing_with(Status::BadArgument)
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(b"POST"))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    module
        .call_proxy_on_response_headers(http_context, 0, false)
        .expect_increment_metric(Some(4), Some(1))
        .expect_get_header_map_pairs(Some(MapType::HttpResponseHeaders))
        .returning(Some(vec![("content-type", "text/event-stream")]))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // Chunk 1: First message arrives (NOT DONE, no usage yet)
    let chunk1 = b"data: {\"id\":\"1\",\"content\":\"Hello\"}\n\n";
    module
        .call_proxy_on_response_body(http_context, chunk1.len() as i32, false)
        .expect_get_buffer_bytes(Some(BufferType::HttpResponseBody))
        .returning(Some(chunk1))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // Chunk 2: Second message arrives (NOT DONE, no usage yet)
    let chunk2 = b"data: {\"id\":\"2\",\"content\":\"World\"}\n\n";
    module
        .call_proxy_on_response_body(http_context, chunk2.len() as i32, false)
        .expect_get_buffer_bytes(Some(BufferType::HttpResponseBody))
        .returning(Some(chunk2))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // Chunk 3: Usage frame arrives (NOT DONE yet) → the SSE parser resolves the
    // field incrementally as soon as it's seen, so the gRPC report is
    // dispatched right away. It isn't a guard, so the chain still continues.
    let chunk3 = b"data: {\"usage\":{\"total_tokens\":42}}\n\n";
    module
        .call_proxy_on_response_body(http_context, chunk3.len() as i32, false)
        .expect_get_buffer_bytes(Some(BufferType::HttpResponseBody))
        .returning(Some(chunk3))
        .expect_grpc_call(
            Some("limitador-cluster"),
            Some("kuadrant.service.ratelimit.v1.RateLimitService"),
            Some("Report"),
            None,
            None,
            Some(5000),
        )
        .returning(Ok(99))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // Chunk 4: DONE frame arrives (still NOT end_of_stream from Envoy's
    // perspective) → the field is already resolved, so nothing left to parse.
    let chunk4 = b"data: [DONE]\n\n";
    module
        .call_proxy_on_response_body(http_context, chunk4.len() as i32, false)
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // Finally: end_of_stream = true with 0 bytes (no new chunk), with the gRPC
    // report still in flight → the filter must pause until it's digested.
    module
        .call_proxy_on_response_body(http_context, 0, true)
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    let grpc_response: [u8; 2] = [8, 1];
    module
        .call_proxy_on_grpc_receive(http_context, 99, grpc_response.len() as i32)
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(&grpc_response))
        .execute_and_expect(ReturnType::None)
        .unwrap();
}

// Documents the accepted "first resolving chunk wins" semantics for an
// ordered candidate list against a streaming (SSE) body: the store task
// commits (and dispatches the downstream report) on the first chunk in which
// *any* candidate resolves, using whichever value that chunk produced. A
// higher-priority candidate arriving in a *later, separate* chunk never gets
// a chance to override it, because the store task has already completed and
// been dropped by then -- unlike the same-chunk case (see
// `both_candidates_present_in_same_event_highest_priority_wins` in
// `sse_body_parser.rs`), where priority order still decides among candidates
// that resolve together.
#[test]
#[serial]
fn it_commits_on_first_resolving_chunk_even_when_a_higher_priority_candidate_arrives_later() {
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
    let report_msg = r#"
        envoy.service.ratelimit.v3.RateLimitRequest {
            domain: "RLS-domain",
            hits_addend: 1u,
            descriptors: [
                envoy.extensions.common.ratelimit.v3.RateLimitDescriptor {
                    entries: (
                        (kuadrant.internal.response.body.total_tokens == 99) ?
                        [envoy.extensions.common.ratelimit.v3.RateLimitDescriptor.Entry {
                            key: "request.method",
                            value: string(request.method)
                        }] :
                        []
                    )
                }
            ]
        }
    "#;
    let cfg = r#"{
        "services": {
            "limitador": {
                "type": "dynamic",
                "endpoint": "limitador-cluster",
                "failureMode": "deny",
                "timeout": "5s",
                "grpcService": "kuadrant.service.ratelimit.v1.RateLimitService",
                "grpcMethod": "Report"
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
                "type": "store",
                "predicate": "true",
                "terminal": false,
                "path": "kuadrant.internal.response.body",
                "value": "{\"total_tokens\": responseBodyJSON(['/usage/total_tokens', '/usageMetadata/totalTokenCount'], 'number')}"
            },
            {
                "type": "grpc",
                "var": "report_response",
                "service": "limitador",
                "predicate": "kuadrant.internal.response.body.total_tokens == 99",
                "terminal": false,
                "isGuard": false,
                "label": "ratelimit_report",
                "messageBuilder": "__REPORT_MSG__",
                "onReply": [
                    {
                        "type": "fail",
                        "predicate": "!has(report_response.overall_code)",
                        "terminal": false,
                        "isGuard": false,
                        "logMessage": "Rate limit report failed: invalid gRPC response"
                    }
                ]
            }
            ]
        }]
    }"#
    .replace("__REPORT_MSG__", &json_escape_cel(report_msg));

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
        .expect_define_metric(
            Some(MetricType::Counter),
            Some("kuadrant.body_extraction_misses"),
        )
        .returning(Some(7))
        .expect_increment_metric(Some(1), Some(1))
        .expect_get_buffer_bytes(Some(BufferType::PluginConfiguration))
        .returning(Some(cfg.as_bytes()))
        .expect_get_log_level()
        .returning(Some(LOG_LEVEL))
        .execute_and_expect(ReturnType::Bool(true))
        .unwrap();

    let http_context = 2;
    module
        .call_proxy_on_context_create(http_context, root_context)
        .expect_get_log_level()
        .returning(Some(LOG_LEVEL))
        .execute_and_expect(ReturnType::None)
        .unwrap();

    module
        .call_proxy_on_request_headers(http_context, 0, false)
        .expect_get_property(Some(vec!["request", "host"]))
        .returning(Some("cars.toystore.com".as_bytes()))
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(None)
        .expect_increment_metric(Some(2), Some(1))
        .expect_get_header_map_pairs(Some(MapType::HttpResponseHeaders))
        .failing_with(Status::BadArgument)
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(b"POST"))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    module
        .call_proxy_on_response_headers(http_context, 0, false)
        .expect_increment_metric(Some(4), Some(1))
        .expect_get_header_map_pairs(Some(MapType::HttpResponseHeaders))
        .returning(Some(vec![("content-type", "text/event-stream")]))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // Chunk 1: only the *lower*-priority candidate (`/usageMetadata/totalTokenCount`,
    // index 1) is present. The field resolves via it immediately, so the report
    // dispatches right away using 99 -- the store task completes and is dropped
    // here, before the higher-priority candidate has ever been seen.
    let chunk1 = b"data: {\"usageMetadata\":{\"totalTokenCount\":99}}\n\n";
    module
        .call_proxy_on_response_body(http_context, chunk1.len() as i32, false)
        .expect_get_buffer_bytes(Some(BufferType::HttpResponseBody))
        .returning(Some(chunk1))
        .expect_grpc_call(
            Some("limitador-cluster"),
            Some("kuadrant.service.ratelimit.v1.RateLimitService"),
            Some("Report"),
            None,
            None,
            Some(5000),
        )
        .returning(Ok(99))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // Chunk 2: the *higher*-priority candidate (`/usage/total_tokens`, index 0)
    // now arrives -- too late. The store task is already gone, so no further
    // body read even happens for it (no `expect_get_buffer_bytes` here), and
    // the committed value stays 99, not 42.
    let chunk2 = b"data: {\"usage\":{\"total_tokens\":42}}\n\n";
    module
        .call_proxy_on_response_body(http_context, chunk2.len() as i32, false)
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // Finally: end_of_stream = true with 0 bytes (no new chunk), with the gRPC
    // report still in flight → the filter must pause until it's digested.
    module
        .call_proxy_on_response_body(http_context, 0, true)
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    let grpc_response: [u8; 2] = [8, 1];
    module
        .call_proxy_on_grpc_receive(http_context, 99, grpc_response.len() as i32)
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(&grpc_response))
        .execute_and_expect(ReturnType::None)
        .unwrap();
}

// SSE analog of `it_does_not_strand_a_sequential_successor_when_a_task_fails`
// (crates/wasm-shim/tests/failures.rs): a `store` action whose ordered
// candidate list never resolves against an SSE body, followed by a
// `sequential` `grpc` report action. Before #426/#427, the sequential
// successor was permanently stranded behind the failed, non-terminal store
// task, leaving the filter paused forever with no gRPC call ever dispatched
// to trigger a resume (a real hang reproduced manually via
// utils/multi_llm_provider's Gemini-shaped SSE fixture). This asserts the
// report action still gets a chance to run.
#[test]
#[serial]
fn it_does_not_strand_a_sequential_successor_when_sse_extraction_fails() {
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
    let report_msg = r#"
        envoy.service.ratelimit.v3.RateLimitRequest {
            domain: "RLS-domain",
            hits_addend: 1u,
            descriptors: [
                envoy.extensions.common.ratelimit.v3.RateLimitDescriptor {
                    entries: [
                        envoy.extensions.common.ratelimit.v3.RateLimitDescriptor.Entry {
                            key: "a",
                            value: string('1')
                        }
                    ]
                }
            ]
        }
    "#;
    let cfg = r#"{
        "services": {
            "limitador": {
                "type": "dynamic",
                "endpoint": "limitador-cluster",
                "failureMode": "deny",
                "timeout": "5s",
                "grpcService": "kuadrant.service.ratelimit.v1.RateLimitService",
                "grpcMethod": "Report"
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
                "type": "store",
                "predicate": "true",
                "terminal": false,
                "path": "kuadrant.internal.response.body",
                "value": "{\"total_tokens\": responseBodyJSON([\"/usage/total_tokens\", \"/usageMetadata/totalTokenCount\"], \"number\")}"
            },
            {
                "type": "grpc",
                "execution": "sequential",
                "var": "report_response",
                "service": "limitador",
                "predicate": "true",
                "terminal": false,
                "isGuard": false,
                "label": "ratelimit_report",
                "messageBuilder": "__REPORT_MSG__",
                "onReply": [
                    {
                        "type": "fail",
                        "predicate": "!has(report_response.overall_code)",
                        "terminal": false,
                        "isGuard": false,
                        "logMessage": "Rate limit report failed: invalid gRPC response"
                    }
                ]
            }
            ]
        }]
    }"#
    .replace("__REPORT_MSG__", &json_escape_cel(report_msg));

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
        .expect_define_metric(
            Some(MetricType::Counter),
            Some("kuadrant.body_extraction_misses"),
        )
        .returning(Some(7))
        .expect_increment_metric(Some(1), Some(1))
        .expect_get_buffer_bytes(Some(BufferType::PluginConfiguration))
        .returning(Some(cfg.as_bytes()))
        .expect_get_log_level()
        .returning(Some(LOG_LEVEL))
        .execute_and_expect(ReturnType::Bool(true))
        .unwrap();

    let http_context = 2;
    module
        .call_proxy_on_context_create(http_context, root_context)
        .expect_get_log_level()
        .returning(Some(LOG_LEVEL))
        .execute_and_expect(ReturnType::None)
        .unwrap();

    module
        .call_proxy_on_request_headers(http_context, 0, false)
        .expect_get_property(Some(vec!["request", "host"]))
        .returning(Some("cars.toystore.com".as_bytes()))
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(None)
        .expect_increment_metric(Some(2), Some(1))
        .expect_get_header_map_pairs(Some(MapType::HttpResponseHeaders))
        .failing_with(Status::BadArgument)
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    module
        .call_proxy_on_response_headers(http_context, 0, false)
        .expect_increment_metric(Some(4), Some(1))
        .expect_get_header_map_pairs(Some(MapType::HttpResponseHeaders))
        .returning(Some(vec![("content-type", "text/event-stream")]))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // Gemini-shaped SSE event: neither configured candidate is present.
    let chunk = b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hi\"}]}}]}\n\n";
    module
        .call_proxy_on_response_body(http_context, chunk.len() as i32, true)
        .expect_get_buffer_bytes(Some(BufferType::HttpResponseBody))
        .returning(Some(chunk))
        .expect_increment_metric(Some(7), Some(1))
        .expect_log(Some(LogLevel::Error), Some("Task failed: \"0\""))
        .expect_grpc_call(
            Some("limitador-cluster"),
            Some("kuadrant.service.ratelimit.v1.RateLimitService"),
            Some("Report"),
            None,
            None,
            Some(5000),
        )
        .returning(Ok(42))
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    let grpc_response: [u8; 2] = [8, 1];
    module
        .call_proxy_on_grpc_receive(http_context, 42, grpc_response.len() as i32)
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(&grpc_response))
        .execute_and_expect(ReturnType::None)
        .unwrap();
}

// Raised in review of #429 (https://github.com/Kuadrant/wasm-shim/pull/429#discussion_r4082725842):
// since the SSE parser now resolves fields incrementally instead of only at
// end_of_stream, a non-guard `grpc` report action dispatched from a mid-stream
// chunk can have its reply -- and therefore pipeline completion -- arrive
// before the upstream has finished sending the rest of the response body.
// Before incremental resolution, pipeline completion could only ever coincide
// with end_of_stream, so this ordering was unreachable.
//
// This asserts wasm-shim's own state machine handles that ordering cleanly:
// `on_grpc_call_response` resumes the response once the pipeline completes,
// and a body chunk that arrives afterwards is treated as a plain pass-through
#[test]
#[serial]
fn it_resumes_when_grpc_reply_arrives_before_upstream_finishes_streaming() {
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
    let report_msg = r#"
        envoy.service.ratelimit.v3.RateLimitRequest {
            domain: "RLS-domain",
            hits_addend: 1u,
            descriptors: [
                envoy.extensions.common.ratelimit.v3.RateLimitDescriptor {
                    entries: [
                        envoy.extensions.common.ratelimit.v3.RateLimitDescriptor.Entry {
                            key: "request.method",
                            value: string(request.method)
                        }
                    ]
                }
            ]
        }
    "#;
    let cfg = r#"{
        "services": {
            "limitador": {
                "type": "dynamic",
                "endpoint": "limitador-cluster",
                "failureMode": "deny",
                "timeout": "5s",
                "grpcService": "kuadrant.service.ratelimit.v1.RateLimitService",
                "grpcMethod": "Report"
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
                "type": "store",
                "predicate": "true",
                "terminal": false,
                "path": "kuadrant.internal.response.body",
                "value": "{\"total_tokens\": responseBodyJSON(['/usage/total_tokens'], 'number')}"
            },
            {
                "type": "grpc",
                "var": "report_response",
                "service": "limitador",
                "predicate": "kuadrant.internal.response.body.total_tokens == 42",
                "terminal": false,
                "isGuard": false,
                "label": "ratelimit_report",
                "messageBuilder": "__REPORT_MSG__",
                "onReply": [
                    {
                        "type": "fail",
                        "predicate": "!has(report_response.overall_code)",
                        "terminal": false,
                        "isGuard": false,
                        "logMessage": "Rate limit report failed: invalid gRPC response"
                    }
                ]
            }
            ]
        }]
    }"#
    .replace("__REPORT_MSG__", &json_escape_cel(report_msg));

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
        .expect_define_metric(
            Some(MetricType::Counter),
            Some("kuadrant.body_extraction_misses"),
        )
        .returning(Some(7))
        .expect_increment_metric(Some(1), Some(1))
        .expect_get_buffer_bytes(Some(BufferType::PluginConfiguration))
        .returning(Some(cfg.as_bytes()))
        .expect_get_log_level()
        .returning(Some(LOG_LEVEL))
        .execute_and_expect(ReturnType::Bool(true))
        .unwrap();

    let http_context = 2;
    module
        .call_proxy_on_context_create(http_context, root_context)
        .expect_get_log_level()
        .returning(Some(LOG_LEVEL))
        .execute_and_expect(ReturnType::None)
        .unwrap();

    module
        .call_proxy_on_request_headers(http_context, 0, false)
        .expect_get_property(Some(vec!["request", "host"]))
        .returning(Some("cars.toystore.com".as_bytes()))
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(None)
        .expect_increment_metric(Some(2), Some(1))
        .expect_get_header_map_pairs(Some(MapType::HttpResponseHeaders))
        .failing_with(Status::BadArgument)
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(b"POST"))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    module
        .call_proxy_on_response_headers(http_context, 0, false)
        .expect_increment_metric(Some(4), Some(1))
        .expect_get_header_map_pairs(Some(MapType::HttpResponseHeaders))
        .returning(Some(vec![("content-type", "text/event-stream")]))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // Chunk 1 (not end_of_stream): the only candidate resolves immediately, so
    // the report dispatches right away. Non-guard, so the filter chain still
    // continues without pausing -- the gRPC call is now in flight while the
    // upstream is still mid-stream.
    let chunk1 = b"data: {\"usage\":{\"total_tokens\":42}}\n\n";
    module
        .call_proxy_on_response_body(http_context, chunk1.len() as i32, false)
        .expect_get_buffer_bytes(Some(BufferType::HttpResponseBody))
        .returning(Some(chunk1))
        .expect_grpc_call(
            Some("limitador-cluster"),
            Some("kuadrant.service.ratelimit.v1.RateLimitService"),
            Some("Report"),
            None,
            None,
            Some(5000),
        )
        .returning(Ok(42))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    // The gRPC reply wins the race, arriving before any further body chunk and
    // long before end_of_stream. With nothing else queued, the pipeline
    // completes right here -- mid-stream -- and resumes the response.
    let grpc_response: [u8; 2] = [8, 1];
    module
        .call_proxy_on_grpc_receive(http_context, 42, grpc_response.len() as i32)
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(&grpc_response))
        .execute_and_expect(ReturnType::None)
        .unwrap();

    // The slow upstream keeps streaming after the pipeline is already gone.
    // No further body host calls should happen at all -- there's nothing left
    // to extract -- just a plain pass-through.
    let chunk2 = b"data: [DONE]\n\n";
    module
        .call_proxy_on_response_body(http_context, chunk2.len() as i32, true)
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();
}
