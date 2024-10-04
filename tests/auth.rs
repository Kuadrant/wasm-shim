use crate::util::wasm_module;
use proxy_wasm_test_framework::tester;
use proxy_wasm_test_framework::types::{Action, BufferType, LogLevel, MapType, ReturnType};
use serial_test::serial;
pub(crate) mod util;

const CONFIG: &str = r#"{
        "extensions": {
            "authorino": {
                "type": "auth",
                "endpoint": "authorino-cluster",
                "failureMode": "deny"
            }
        },
        "policies": [
        {
            "name": "some-name",
            "hostnames": ["*.toystore.com", "example.com"],
            "rules": [
            {
                "conditions": [
                {
                    "allOf": [
                    {
                        "selector": "request.url_path",
                        "operator": "startswith",
                        "value": "/admin/toy"
                    },
                    {
                        "selector": "request.host",
                        "operator": "eq",
                        "value": "cars.toystore.com"
                    },
                    {
                        "selector": "request.method",
                        "operator": "eq",
                        "value": "POST"
                    }]
                }],
                "actions": [
                {
                    "extension": "authorino",
                    "scope": "authconfig-A"
                }]
            }]
        }]
    }"#;

#[test]
#[serial]
fn it_auths() {
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
        .expect_log(Some(LogLevel::Debug), Some("#2 policy selected some-name"))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property:  selector: request.url_path path: [\"request\", \"url_path\"]"),
        )
        .expect_get_property(Some(vec!["request", "url_path"]))
        .returning(Some("/admin/toy".as_bytes()))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property:  selector: request.host path: [\"request\", \"host\"]"),
        )
        .expect_get_property(Some(vec!["request", "host"]))
        .returning(Some("cars.toystore.com".as_bytes()))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property:  selector: request.method path: [\"request\", \"method\"]"),
        )
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some("POST".as_bytes()))
        // retrieving properties for CheckRequest
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(None)
        .expect_get_property(Some(vec!["request", "host"]))
        .returning(Some("cars.toystore.com".as_bytes()))
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some("GET".as_bytes()))
        .expect_get_property(Some(vec!["request", "scheme"]))
        .returning(Some("http".as_bytes()))
        .expect_get_property(Some(vec!["request", "path"]))
        .returning(Some("/admin/toy".as_bytes()))
        .expect_get_property(Some(vec!["request", "protocol"]))
        .returning(Some("HTTP".as_bytes()))
        .expect_get_property(Some(vec!["request", "time"]))
        .returning(None)
        .expect_get_property(Some(vec!["destination", "address"]))
        .returning(Some("127.0.0.1:8000".as_bytes()))
        .expect_get_property(Some(vec!["destination", "port"]))
        .returning(Some("8000".as_bytes()))
        .expect_get_property(Some(vec!["source", "address"]))
        .returning(Some("127.0.0.1:45000".as_bytes()))
        .expect_get_property(Some(vec!["source", "port"]))
        .returning(Some("45000".as_bytes()))
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
            Some(&[
                10, 217, 1, 10, 23, 10, 21, 10, 19, 18, 15, 49, 50, 55, 46, 48, 46, 48, 46, 49, 58,
                52, 53, 48, 48, 48, 24, 0, 18, 22, 10, 20, 10, 18, 18, 14, 49, 50, 55, 46, 48, 46,
                48, 46, 49, 58, 56, 48, 48, 48, 24, 0, 34, 141, 1, 10, 0, 18, 136, 1, 18, 3, 71,
                69, 84, 26, 30, 10, 10, 58, 97, 117, 116, 104, 111, 114, 105, 116, 121, 18, 16, 97,
                98, 105, 95, 116, 101, 115, 116, 95, 104, 97, 114, 110, 101, 115, 115, 26, 14, 10,
                7, 58, 109, 101, 116, 104, 111, 100, 18, 3, 71, 69, 84, 26, 38, 10, 5, 58, 112, 97,
                116, 104, 18, 29, 47, 100, 101, 102, 97, 117, 108, 116, 47, 114, 101, 113, 117,
                101, 115, 116, 47, 104, 101, 97, 100, 101, 114, 115, 47, 112, 97, 116, 104, 34, 10,
                47, 97, 100, 109, 105, 110, 47, 116, 111, 121, 42, 17, 99, 97, 114, 115, 46, 116,
                111, 121, 115, 116, 111, 114, 101, 46, 99, 111, 109, 50, 4, 104, 116, 116, 112, 82,
                4, 72, 84, 84, 80, 82, 20, 10, 4, 104, 111, 115, 116, 18, 12, 97, 117, 116, 104,
                99, 111, 110, 102, 105, 103, 45, 65, 90, 0,
            ]),
            Some(5000),
        )
        .returning(Some(42))
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 initiated gRPC call (id# 42)"),
        )
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    // TODO: response containing dynamic metadata
    // set_property is panicking with proxy-wasm-test-framework
    // let grpc_response: [u8; 41] = [
    //     10, 0, 34, 35, 10, 33, 10, 8, 105, 100, 101, 110, 116, 105, 116, 121, 18, 21, 42, 19, 10,
    //     17, 10, 6, 117, 115, 101, 114, 105, 100, 18, 7, 26, 5, 97, 108, 105, 99, 101, 26, 0,
    // ];
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
            Some("#2 process_auth_grpc_response: received OkHttpResponse"),
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
fn it_denies() {
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
        .expect_log(Some(LogLevel::Debug), Some("#2 policy selected some-name"))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property:  selector: request.url_path path: [\"request\", \"url_path\"]"),
        )
        .expect_get_property(Some(vec!["request", "url_path"]))
        .returning(Some("/admin/toy".as_bytes()))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property:  selector: request.host path: [\"request\", \"host\"]"),
        )
        .expect_get_property(Some(vec!["request", "host"]))
        .returning(Some("cars.toystore.com".as_bytes()))
        .expect_log(
            Some(LogLevel::Debug),
            Some("get_property:  selector: request.method path: [\"request\", \"method\"]"),
        )
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some("POST".as_bytes()))
        // retrieving properties for CheckRequest
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(None)
        .expect_get_property(Some(vec!["request", "host"]))
        .returning(Some("cars.toystore.com".as_bytes()))
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some("GET".as_bytes()))
        .expect_get_property(Some(vec!["request", "scheme"]))
        .returning(Some("http".as_bytes()))
        .expect_get_property(Some(vec!["request", "path"]))
        .returning(Some("/admin/toy".as_bytes()))
        .expect_get_property(Some(vec!["request", "protocol"]))
        .returning(Some("HTTP".as_bytes()))
        .expect_get_property(Some(vec!["request", "time"]))
        .returning(None)
        .expect_get_property(Some(vec!["destination", "address"]))
        .returning(Some("127.0.0.1:8000".as_bytes()))
        .expect_get_property(Some(vec!["destination", "port"]))
        .returning(Some("8000".as_bytes()))
        .expect_get_property(Some(vec!["source", "address"]))
        .returning(Some("127.0.0.1:45000".as_bytes()))
        .expect_get_property(Some(vec!["source", "port"]))
        .returning(Some("45000".as_bytes()))
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
            Some(&[
                10, 217, 1, 10, 23, 10, 21, 10, 19, 18, 15, 49, 50, 55, 46, 48, 46, 48, 46, 49, 58,
                52, 53, 48, 48, 48, 24, 0, 18, 22, 10, 20, 10, 18, 18, 14, 49, 50, 55, 46, 48, 46,
                48, 46, 49, 58, 56, 48, 48, 48, 24, 0, 34, 141, 1, 10, 0, 18, 136, 1, 18, 3, 71,
                69, 84, 26, 30, 10, 10, 58, 97, 117, 116, 104, 111, 114, 105, 116, 121, 18, 16, 97,
                98, 105, 95, 116, 101, 115, 116, 95, 104, 97, 114, 110, 101, 115, 115, 26, 14, 10,
                7, 58, 109, 101, 116, 104, 111, 100, 18, 3, 71, 69, 84, 26, 38, 10, 5, 58, 112, 97,
                116, 104, 18, 29, 47, 100, 101, 102, 97, 117, 108, 116, 47, 114, 101, 113, 117,
                101, 115, 116, 47, 104, 101, 97, 100, 101, 114, 115, 47, 112, 97, 116, 104, 34, 10,
                47, 97, 100, 109, 105, 110, 47, 116, 111, 121, 42, 17, 99, 97, 114, 115, 46, 116,
                111, 121, 115, 116, 111, 114, 101, 46, 99, 111, 109, 50, 4, 104, 116, 116, 112, 82,
                4, 72, 84, 84, 80, 82, 20, 10, 4, 104, 111, 115, 116, 18, 12, 97, 117, 116, 104,
                99, 111, 110, 102, 105, 103, 45, 65, 90, 0,
            ]),
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
            Some("#2 process_auth_grpc_response: received DeniedHttpResponse"),
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
