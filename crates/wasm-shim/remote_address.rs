use crate::util::common::{wasm_module, LOG_LEVEL};
use crate::util::data;
use proxy_wasm_test_framework::tester;
use proxy_wasm_test_framework::types::{
    Action, BufferType, LogLevel, MapType, MetricType, ReturnType,
};
use serial_test::serial;

pub mod util;

#[test]
#[serial]
fn it_limits_based_on_source_address() {
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
                    "hostnames": ["cars.toystore.com"],
                    "predicates" : [
                        "source.remote_address != '50.0.0.1'"
                    ]
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
                                        "key": "source.remote_address",
                                        "value": "source.remote_address"
                                    }
                                }
                            ]
                        }]
                    }
                ]
            }
        ]
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
        .returning(Some(data::request::HOST))
        // retrieving properties for conditions
        .expect_get_property(Some(vec!["source", "address"]))
        .returning(Some(data::source::ADDRESS))
        // retrieving tracing headers
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(None)
        .expect_increment_metric(Some(2), Some(1))
        .expect_grpc_call(
            Some("limitador-cluster"),
            Some("envoy.service.ratelimit.v3.RateLimitService"),
            Some("ShouldRateLimit"),
            None,
            Some(&[
                10, 10, 82, 76, 83, 45, 100, 111, 109, 97, 105, 110, 18, 36, 10, 34, 10, 21, 115,
                111, 117, 114, 99, 101, 46, 114, 101, 109, 111, 116, 101, 95, 97, 100, 100, 114,
                101, 115, 115, 18, 9, 49, 50, 55, 46, 48, 46, 48, 46, 49, 24, 1,
            ]),
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

    module
        .call_proxy_on_response_headers(http_context, 0, false)
        .expect_increment_metric(Some(4), Some(1))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();
}
