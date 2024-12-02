use crate::util::common::wasm_module;
use crate::util::data;
use proxy_wasm_test_framework::tester;
use proxy_wasm_test_framework::types::{Action, BufferType, LogLevel, MapType, ReturnType};
use serial_test::serial;

pub mod util;

const CONFIG: &str = r#"{
    "services": {
        "authorino": {
            "type": "auth",
            "endpoint": "authorino-cluster",
            "failureMode": "deny",
            "timeout": "5s"
        },
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
                "request.url_path.startsWith('/admin/toy')",
                "request.host == 'cars.toystore.com'",
                "request.method == 'POST'"
            ]
        },
        "actions": [
        {
            "service": "authorino",
            "scope": "authconfig-A"
        },
        {
            "service": "limitador",
            "scope": "RLS-domain",
            "data": [
            {
                "static": {
                    "key": "admin",
                    "value": "1"
                }
            }]
        }]
    }]
}"#;

#[test]
#[serial]
fn it_performs_authenticated_rate_limiting() {
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

    module
        .call_proxy_on_context_create(root_context, 0)
        .expect_log(Some(LogLevel::Info), Some("#1 set_root_context"))
        .execute_and_expect(ReturnType::None)
        .unwrap();

    module
        .call_proxy_on_configure(root_context, 0)
        .expect_log(Some(LogLevel::Info), Some("#1 on_configure"))
        .expect_get_buffer_bytes(Some(BufferType::PluginConfiguration))
        .returning(Some(CONFIG.as_bytes()))
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
            Some("get_property: path: [\"request\", \"host\"]"),
        )
        .expect_get_property(Some(vec!["request", "host"]))
        .returning(Some(data::request::HOST))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"method\"]"),
        )
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(data::request::method::POST))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 action_set selected some-name"),
        )
        // retrieving properties for CheckRequest
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(None)
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"host\"]"),
        )
        .expect_get_property(Some(vec!["request", "host"]))
        .returning(Some(data::request::HOST))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"method\"]"),
        )
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(data::request::method::GET))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"scheme\"]"),
        )
        .expect_get_property(Some(vec!["request", "scheme"]))
        .returning(Some(data::request::scheme::HTTP))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"path\"]"),
        )
        .expect_get_property(Some(vec!["request", "path"]))
        .returning(Some(data::request::path::ADMIN_TOY))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"protocol\"]"),
        )
        .expect_get_property(Some(vec!["request", "protocol"]))
        .returning(Some(data::request::protocol::HTTP_1_1))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"time\"]"),
        )
        .expect_get_property(Some(vec!["request", "time"]))
        .returning(Some(data::request::TIME))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"destination\", \"address\"]"),
        )
        .expect_get_property(Some(vec!["destination", "address"]))
        .returning(Some(data::destination::ADDRESS))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"destination\", \"port\"]"),
        )
        .expect_get_property(Some(vec!["destination", "port"]))
        .returning(Some(data::destination::port::P_8000))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"source\", \"address\"]"),
        )
        .expect_get_property(Some(vec!["source", "address"]))
        .returning(Some(data::source::ADDRESS))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"source\", \"port\"]"),
        )
        .expect_get_property(Some(vec!["source", "port"]))
        .returning(Some(data::source::port::P_45000))
        // retrieving tracing headers
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("traceparent"))
        .returning(None)
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("tracestate"))
        .returning(None)
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("baggage"))
        .returning(None)
        .expect_grpc_call(
            Some("authorino-cluster"),
            Some("envoy.service.auth.v3.Authorization"),
            Some("Check"),
            Some(&[0, 0, 0, 0]),
            None,
            Some(5000),
        )
        .returning(Some(42))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 initiated gRPC call (id# 42)"),
        )
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    let grpc_response: [u8; 6] = [10, 0, 34, 0, 26, 0];
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
            Some("process_auth_grpc_response: received OkHttpResponse"),
        )
        .expect_grpc_call(
            Some("limitador-cluster"),
            Some("envoy.service.ratelimit.v3.RateLimitService"),
            Some("ShouldRateLimit"),
            Some(&[0, 0, 0, 0]),
            None,
            Some(5000),
        )
        .returning(Some(43))
        .execute_and_expect(ReturnType::None)
        .unwrap();

    let grpc_response: [u8; 2] = [8, 1];
    module
        .call_proxy_on_grpc_receive(http_context, 43, grpc_response.len() as i32)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 on_grpc_call_response: received gRPC call response: token: 43, status: 0"),
        )
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(&grpc_response))
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
fn unauthenticated_does_not_ratelimit() {
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

    module
        .call_proxy_on_context_create(root_context, 0)
        .expect_log(Some(LogLevel::Info), Some("#1 set_root_context"))
        .execute_and_expect(ReturnType::None)
        .unwrap();

    module
        .call_proxy_on_configure(root_context, 0)
        .expect_log(Some(LogLevel::Info), Some("#1 on_configure"))
        .expect_get_buffer_bytes(Some(BufferType::PluginConfiguration))
        .returning(Some(CONFIG.as_bytes()))
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
            Some("get_property: path: [\"request\", \"host\"]"),
        )
        .expect_get_property(Some(vec!["request", "host"]))
        .returning(Some(data::request::HOST))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"method\"]"),
        )
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(data::request::method::POST))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 action_set selected some-name"),
        )
        // retrieving properties for CheckRequest
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(None)
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"host\"]"),
        )
        .expect_get_property(Some(vec!["request", "host"]))
        .returning(Some(data::request::HOST))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"method\"]"),
        )
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(data::request::method::GET))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"scheme\"]"),
        )
        .expect_get_property(Some(vec!["request", "scheme"]))
        .returning(Some(data::request::scheme::HTTP))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"path\"]"),
        )
        .expect_get_property(Some(vec!["request", "path"]))
        .returning(Some(data::request::path::ADMIN_TOY))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"protocol\"]"),
        )
        .expect_get_property(Some(vec!["request", "protocol"]))
        .returning(Some(data::request::protocol::HTTP_1_1))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"time\"]"),
        )
        .expect_get_property(Some(vec!["request", "time"]))
        .returning(Some(data::request::TIME))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"destination\", \"address\"]"),
        )
        .expect_get_property(Some(vec!["destination", "address"]))
        .returning(Some(data::destination::ADDRESS))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"destination\", \"port\"]"),
        )
        .expect_get_property(Some(vec!["destination", "port"]))
        .returning(Some(data::destination::port::P_8000))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"source\", \"address\"]"),
        )
        .expect_get_property(Some(vec!["source", "address"]))
        .returning(Some(data::source::ADDRESS))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"source\", \"port\"]"),
        )
        .expect_get_property(Some(vec!["source", "port"]))
        .returning(Some(data::source::port::P_45000))
        // retrieving tracing headers
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("traceparent"))
        .returning(None)
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("tracestate"))
        .returning(None)
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("baggage"))
        .returning(None)
        .expect_grpc_call(
            Some("authorino-cluster"),
            Some("envoy.service.auth.v3.Authorization"),
            Some("Check"),
            Some(&[0, 0, 0, 0]),
            None,
            Some(5000),
        )
        .returning(Some(42))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 initiated gRPC call (id# 42)"),
        )
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    let grpc_response: [u8; 108] = [
        10, 2, 8, 16, 18, 102, 10, 3, 8, 145, 3, 18, 50, 10, 48, 10, 16, 87, 87, 87, 45, 65, 117,
        116, 104, 101, 110, 116, 105, 99, 97, 116, 101, 18, 28, 65, 80, 73, 75, 69, 89, 32, 114,
        101, 97, 108, 109, 61, 34, 97, 112, 105, 45, 107, 101, 121, 45, 117, 115, 101, 114, 115,
        34, 18, 43, 10, 41, 10, 17, 88, 45, 69, 120, 116, 45, 65, 117, 116, 104, 45, 82, 101, 97,
        115, 111, 110, 18, 20, 99, 114, 101, 100, 101, 110, 116, 105, 97, 108, 32, 110, 111, 116,
        32, 102, 111, 117, 110, 100,
    ];
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
            Some("process_auth_grpc_response: received DeniedHttpResponse"),
        )
        .expect_send_local_response(
            Some(401),
            None,
            Some(vec![
                ("WWW-Authenticate", "APIKEY realm=\"api-key-users\""),
                ("X-Ext-Auth-Reason", "credential not found"),
            ]),
            None,
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
fn authenticated_one_ratelimit_action_matches() {
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

    let cfg = r#"{
        "services": {
            "authorino": {
                "type": "auth",
                "endpoint": "authorino-cluster",
                "failureMode": "deny",
                "timeout": "5s"
            },
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
                    "request.url_path.startsWith('/admin/toy')",
                    "request.host == 'cars.toystore.com'",
                    "request.method == 'POST'"
                ]
            },
            "actions": [
            {
                "service": "authorino",
                "scope": "authconfig-A"
            },
            {
                "service": "limitador",
                "scope": "RLS-domain",
                "predicates" : [
                    "source.address == '127.0.0.1:80'"
                ],
                "data": [
                {
                    "static": {
                        "key": "me",
                        "value": "1"
                    }
                }]
            },
            {
                "service": "limitador",
                "scope": "RLS-domain",
                "predicates" : [
                    "source.address != '127.0.0.1:80'"
                ],
                "data": [
                {
                    "static": {
                        "key": "other",
                        "value": "1"
                    }
                }]
            }
            ]
        }]
    }"#;

    module
        .call_start()
        .execute_and_expect(ReturnType::None)
        .unwrap();

    let root_context = 1;

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
            Some("get_property: path: [\"request\", \"host\"]"),
        )
        .expect_get_property(Some(vec!["request", "host"]))
        .returning(Some(data::request::HOST))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"method\"]"),
        )
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(data::request::method::POST))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 action_set selected some-name"),
        )
        // retrieving properties for CheckRequest
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(None)
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"host\"]"),
        )
        .expect_get_property(Some(vec!["request", "host"]))
        .returning(Some(data::request::HOST))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"method\"]"),
        )
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(data::request::method::GET))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"scheme\"]"),
        )
        .expect_get_property(Some(vec!["request", "scheme"]))
        .returning(Some(data::request::scheme::HTTP))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"path\"]"),
        )
        .expect_get_property(Some(vec!["request", "path"]))
        .returning(Some(data::request::path::ADMIN_TOY))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"protocol\"]"),
        )
        .expect_get_property(Some(vec!["request", "protocol"]))
        .returning(Some(data::request::protocol::HTTP_1_1))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"request\", \"time\"]"),
        )
        .expect_get_property(Some(vec!["request", "time"]))
        .returning(Some(data::request::TIME))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"destination\", \"address\"]"),
        )
        .expect_get_property(Some(vec!["destination", "address"]))
        .returning(Some(data::destination::ADDRESS))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"destination\", \"port\"]"),
        )
        .expect_get_property(Some(vec!["destination", "port"]))
        .returning(Some(data::destination::port::P_8000))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"source\", \"address\"]"),
        )
        .expect_get_property(Some(vec!["source", "address"]))
        .returning(Some(data::source::ADDRESS))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"source\", \"port\"]"),
        )
        .expect_get_property(Some(vec!["source", "port"]))
        .returning(Some(data::source::port::P_45000))
        // retrieving tracing headers
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("traceparent"))
        .returning(None)
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("tracestate"))
        .returning(None)
        .expect_get_header_map_value(Some(MapType::HttpRequestHeaders), Some("baggage"))
        .returning(None)
        .expect_grpc_call(
            Some("authorino-cluster"),
            Some("envoy.service.auth.v3.Authorization"),
            Some("Check"),
            Some(&[0, 0, 0, 0]),
            None,
            Some(5000),
        )
        .returning(Some(42))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 initiated gRPC call (id# 42)"),
        )
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    let grpc_response: [u8; 6] = [10, 0, 34, 0, 26, 0];
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
            Some("process_auth_grpc_response: received OkHttpResponse"),
        )
        // conditions checks
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"source\", \"address\"]"),
        )
        .expect_get_property(Some(vec!["source", "address"]))
        .returning(Some("1.2.3.4:80".as_bytes()))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property: path: [\"source\", \"address\"]"),
        )
        .expect_get_property(Some(vec!["source", "address"]))
        .returning(Some("1.2.3.4:80".as_bytes()))
        .expect_grpc_call(
            Some("limitador-cluster"),
            Some("envoy.service.ratelimit.v3.RateLimitService"),
            Some("ShouldRateLimit"),
            Some(&[0, 0, 0, 0]),
            None,
            Some(5000),
        )
        .returning(Some(43))
        .execute_and_expect(ReturnType::None)
        .unwrap();

    let grpc_response: [u8; 2] = [8, 1];
    module
        .call_proxy_on_grpc_receive(http_context, 43, grpc_response.len() as i32)
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 on_grpc_call_response: received gRPC call response: token: 43, status: 0"),
        )
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(&grpc_response))
        .execute_and_expect(ReturnType::None)
        .unwrap();

    module
        .call_proxy_on_response_headers(http_context, 0, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_response_headers"))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();
}
