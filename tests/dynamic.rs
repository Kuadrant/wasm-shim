use crate::util::common::{wasm_module, LOG_LEVEL};
use crate::util::data;
use proxy_wasm_test_framework::tester;
use proxy_wasm_test_framework::types::{
    Action, BufferType, LogLevel, MapType, MetricType, ReturnType,
};
use serial_test::serial;

pub mod util;

#[allow(clippy::unwrap_used)]
fn configure_dynamic_service(module: &mut tester::Tester, root_context: i32, cfg: &str) {
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
        .expect_get_log_level()
        .returning(Some(LOG_LEVEL))
        .expect_grpc_call(
            Some("descriptor-service-cluster"),
            Some("kuadrant.v1.DescriptorService"),
            Some("GetServiceDescriptors"),
            None,
            None,
            Some(1000),
        )
        .returning(Ok(42))
        .expect_set_tick_period_millis(Some(2000))
        .execute_and_expect(ReturnType::Bool(true))
        .unwrap();

    let descriptor_response = data::descriptor_response::TEST_SERVICE;
    module
        .call_proxy_on_grpc_receive(root_context, 42, descriptor_response.len() as i32)
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(descriptor_response))
        .execute_and_expect(ReturnType::None)
        .unwrap();
}

const DESCRIPTOR_FETCH_CONFIG: &str = r#"{
    "descriptorService": "descriptor-service-cluster",
    "services": {
        "dynamic-ratelimit": {
            "type": "dynamic",
            "endpoint": "limitador-cluster",
            "failureMode": "deny",
            "timeout": "5s",
            "grpcService": "test.TestService",
            "grpcMethod": "TestMethod"
        }
    },
    "actionSets": []
}"#;

const DENY_CONFIG: &str = r#"{
    "descriptorService": "descriptor-service-cluster",
    "services": {
        "my-service": {
            "type": "dynamic",
            "endpoint": "limitador-cluster",
            "failureMode": "deny",
            "timeout": "5s",
            "grpcService": "test.TestService",
            "grpcMethod": "TestMethod"
        }
    },
    "actionSets": [{
        "name": "deny-test",
        "routeRuleConditions": {
            "hostnames": ["*.toystore.com"]
        },
        "actions": [{
            "type": "grpc",
            "name": "my_check",
            "service": "my-service",
            "predicate": "true",
            "terminal": false,
            "messageBuilder": "test.TestRequest{message: 'hello'}",
            "onReply": [{
                "type": "deny",
                "predicate": "my_check.result == 'denied'",
                "denyWith": "DenyResponse{status: 429u, body: 'Too Many Requests', headers: [['x-ratelimit-reason', my_check.result]]}",
                "terminal": true
            }]
        }]
    }]
}"#;

#[test]
#[serial]
fn it_fetches_descriptors_on_configure() {
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
    configure_dynamic_service(&mut module, root_context, DESCRIPTOR_FETCH_CONFIG);

    let http_context = 2;
    module
        .call_proxy_on_context_create(http_context, root_context)
        .expect_get_log_level()
        .returning(Some(LOG_LEVEL))
        .execute_and_expect(ReturnType::None)
        .unwrap();
}

#[test]
#[serial]
fn it_allows_when_deny_predicate_is_false() {
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
    configure_dynamic_service(&mut module, root_context, DENY_CONFIG);

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
        .returning(Some(data::request::HOST))
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(None)
        .expect_increment_metric(Some(2), Some(1))
        .expect_grpc_call(
            Some("limitador-cluster"),
            Some("test.TestService"),
            Some("TestMethod"),
            None,
            None,
            Some(5000),
        )
        .returning(Ok(43))
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    let grpc_response = data::dynamic_response::RESULT_ALLOWED;
    module
        .call_proxy_on_grpc_receive(http_context, 43, grpc_response.len() as i32)
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(grpc_response))
        .execute_and_expect(ReturnType::None)
        .unwrap();

    module
        .call_proxy_on_response_headers(http_context, 0, false)
        .expect_increment_metric(Some(4), Some(1))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();
}

#[test]
#[serial]
fn it_denies_when_deny_predicate_is_true() {
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
    configure_dynamic_service(&mut module, root_context, DENY_CONFIG);

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
        .returning(Some(data::request::HOST))
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(None)
        .expect_increment_metric(Some(2), Some(1))
        .expect_grpc_call(
            Some("limitador-cluster"),
            Some("test.TestService"),
            Some("TestMethod"),
            None,
            None,
            Some(5000),
        )
        .returning(Ok(43))
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    let grpc_response = data::dynamic_response::RESULT_DENIED;
    module
        .call_proxy_on_grpc_receive(http_context, 43, grpc_response.len() as i32)
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(grpc_response))
        .expect_increment_metric(Some(5), Some(1))
        .expect_send_local_response(
            Some(429),
            Some("Too Many Requests"),
            Some(vec![("x-ratelimit-reason", "denied")]),
            None,
        )
        .execute_and_expect(ReturnType::None)
        .unwrap();
}

#[test]
#[serial]
fn it_skips_grpc_action_when_predicate_is_false() {
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
        "descriptorService": "descriptor-service-cluster",
        "services": {
            "my-service": {
                "type": "dynamic",
                "endpoint": "limitador-cluster",
                "failureMode": "deny",
                "timeout": "5s",
                "grpcService": "test.TestService",
                "grpcMethod": "TestMethod"
            }
        },
        "actionSets": [{
            "name": "predicate-test",
            "routeRuleConditions": {
                "hostnames": ["*.toystore.com"]
            },
            "actions": [{
                "type": "grpc",
                "name": "my_check",
                "service": "my-service",
                "predicate": "request.method == 'DELETE'",
                "terminal": false,
                "messageBuilder": "test.TestRequest{message: 'hello'}",
                "onReply": [{
                    "type": "deny",
                    "predicate": "true",
                    "denyWith": "DenyResponse{status: 403u}",
                    "terminal": true
                }]
            }]
        }]
    }"#;
    configure_dynamic_service(&mut module, root_context, cfg);

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
        .returning(Some(data::request::HOST))
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(None)
        .expect_increment_metric(Some(2), Some(1))
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(data::request::method::GET))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();

    module
        .call_proxy_on_response_headers(http_context, 0, false)
        .expect_increment_metric(Some(4), Some(1))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();
}

#[test]
#[serial]
fn it_sets_headers_from_grpc_response() {
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
        "descriptorService": "descriptor-service-cluster",
        "services": {
            "my-service": {
                "type": "dynamic",
                "endpoint": "limitador-cluster",
                "failureMode": "deny",
                "timeout": "5s",
                "grpcService": "test.TestService",
                "grpcMethod": "TestMethod"
            }
        },
        "actionSets": [{
            "name": "headers-test",
            "routeRuleConditions": {
                "hostnames": ["*.toystore.com"]
            },
            "actions": [{
                "type": "grpc",
                "name": "my_check",
                "service": "my-service",
                "predicate": "true",
                "terminal": false,
                "messageBuilder": "test.TestRequest{message: 'hello'}",
                "onReply": [{
                    "type": "headers",
                    "predicate": "true",
                    "terminal": false,
                    "target": "request",
                    "headers": "[['x-result', my_check.result], ['x-custom', 'static-value']]"
                }]
            }]
        }]
    }"#;
    configure_dynamic_service(&mut module, root_context, cfg);

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
        .returning(Some(data::request::HOST))
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(None)
        .expect_increment_metric(Some(2), Some(1))
        .expect_grpc_call(
            Some("limitador-cluster"),
            Some("test.TestService"),
            Some("TestMethod"),
            None,
            None,
            Some(5000),
        )
        .returning(Ok(43))
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    let grpc_response = data::dynamic_response::RESULT_CHECK_PASSED;
    module
        .call_proxy_on_grpc_receive(http_context, 43, grpc_response.len() as i32)
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(grpc_response))
        .expect_set_header_map_pairs(None, None)
        .execute_and_expect(ReturnType::None)
        .unwrap();

    module
        .call_proxy_on_response_headers(http_context, 0, false)
        .expect_increment_metric(Some(4), Some(1))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();
}

#[test]
#[serial]
fn it_stores_data_from_grpc_response() {
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
        "descriptorService": "descriptor-service-cluster",
        "services": {
            "my-service": {
                "type": "dynamic",
                "endpoint": "limitador-cluster",
                "failureMode": "deny",
                "timeout": "5s",
                "grpcService": "test.TestService",
                "grpcMethod": "TestMethod"
            }
        },
        "actionSets": [{
            "name": "store-test",
            "routeRuleConditions": {
                "hostnames": ["*.toystore.com"]
            },
            "actions": [{
                "type": "grpc",
                "name": "my_check",
                "service": "my-service",
                "predicate": "true",
                "terminal": false,
                "messageBuilder": "test.TestRequest{message: 'hello'}",
                "onReply": [{
                    "type": "store",
                    "predicate": "true",
                    "terminal": false,
                    "data": [{
                        "path": "check.result",
                        "value": "my_check.result"
                    }]
                }]
            }]
        }]
    }"#;
    configure_dynamic_service(&mut module, root_context, cfg);

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
        .returning(Some(data::request::HOST))
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(None)
        .expect_increment_metric(Some(2), Some(1))
        .expect_grpc_call(
            Some("limitador-cluster"),
            Some("test.TestService"),
            Some("TestMethod"),
            None,
            None,
            Some(5000),
        )
        .returning(Ok(43))
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    let grpc_response = data::dynamic_response::RESULT_STORED_VALUE;
    module
        .call_proxy_on_grpc_receive(http_context, 43, grpc_response.len() as i32)
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(grpc_response))
        .expect_set_property(Some(vec!["kuadrant.check.result"]), Some(b"stored_value"))
        .execute_and_expect(ReturnType::None)
        .unwrap();

    module
        .call_proxy_on_response_headers(http_context, 0, false)
        .expect_increment_metric(Some(4), Some(1))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();
}
