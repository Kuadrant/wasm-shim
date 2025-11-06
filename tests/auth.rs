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
            Some("Getting property: `request.method`"),
        )
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(data::request::method::POST))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Selected blueprint some-name for hostname: cars.toystore.com"),
        )
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 pipeline built successfully"),
        )
        // retrieving properties for CheckRequest
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting map: `HttpRequestHeaders`"),
        )
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(None)
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `request.scheme`"),
        )
        .expect_get_property(Some(vec!["request", "scheme"]))
        .returning(Some(data::request::scheme::HTTP))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `request.path`"),
        )
        .expect_get_property(Some(vec!["request", "path"]))
        .returning(Some(data::request::path::ADMIN_TOY))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `request.protocol`"),
        )
        .expect_get_property(Some(vec!["request", "protocol"]))
        .returning(Some(data::request::protocol::HTTP_1_1))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `request.time`"),
        )
        .expect_get_property(Some(vec!["request", "time"]))
        .returning(Some(data::request::TIME))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `destination.address`"),
        )
        .expect_get_property(Some(vec!["destination", "address"]))
        .returning(Some(data::destination::ADDRESS))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `destination.port`"),
        )
        .expect_get_property(Some(vec!["destination", "port"]))
        .returning(Some(data::destination::port::P_8000))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `source.address`"),
        )
        .expect_get_property(Some(vec!["source", "address"]))
        .returning(Some(data::source::ADDRESS))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `source.port`"),
        )
        .expect_get_property(Some(vec!["source", "port"]))
        .returning(Some(data::source::port::P_45000))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Dispatching gRPC call to authorino-cluster/envoy.service.auth.v3.Authorization.Check, timeout: 5s"),
        )
        .expect_grpc_call(
            Some("authorino-cluster"),
            Some("envoy.service.auth.v3.Authorization"),
            Some("Check"),
            Some(&[0, 0, 0, 0]),
            None,
            Some(5000),
        )
        .returning(Ok(42))
        .expect_log(
            Some(LogLevel::Debug),
            Some("gRPC call dispatched successfully, token_id: 42"),
        )
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    // TODO: response containing dynamic metadata
    // set_property is panicking with proxy-wasm-test-framework
    // because the `expect_set_property` is not yet implemented neither on original repo nor our fork
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
        .expect_log(
            Some(LogLevel::Debug),
            Some(format!("Getting gRPC response, size: {} bytes", grpc_response.len()).as_str()),
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
fn it_passes_request_data() {
    let cfg = r#"{
        "requestData": {
            "foo": "string(2 + 3)",
            "bar": "auth.identity.name"
        },
        "services": {
            "authorino": {
                "type": "auth",
                "endpoint": "authorino-cluster",
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
            }]
        }]
    }"#;

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
            Some("Getting property: `request.method`"),
        )
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(data::request::method::POST))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Selected blueprint some-name for hostname: cars.toystore.com"),
        )
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 pipeline built successfully"),
        )
        // retrieving properties for CheckRequest
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting map: `HttpRequestHeaders`"),
        )
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(None)
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `request.scheme`"),
        )
        .expect_get_property(Some(vec!["request", "scheme"]))
        .returning(Some(data::request::scheme::HTTP))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `request.path`"),
        )
        .expect_get_property(Some(vec!["request", "path"]))
        .returning(Some(data::request::path::ADMIN_TOY))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `request.protocol`"),
        )
        .expect_get_property(Some(vec!["request", "protocol"]))
        .returning(Some(data::request::protocol::HTTP_1_1))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `request.time`"),
        )
        .expect_get_property(Some(vec!["request", "time"]))
        .returning(Some(data::request::TIME))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `destination.address`"),
        )
        .expect_get_property(Some(vec!["destination", "address"]))
        .returning(Some(data::destination::ADDRESS))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `destination.port`"),
        )
        .expect_get_property(Some(vec!["destination", "port"]))
        .returning(Some(data::destination::port::P_8000))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `source.address`"),
        )
        .expect_get_property(Some(vec!["source", "address"]))
        .returning(Some(data::source::ADDRESS))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `source.port`"),
        )
        .expect_get_property(Some(vec!["source", "port"]))
        .returning(Some(data::source::port::P_45000))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `filter_state.wasm\\.kuadrant\\.auth\\.identity\\.name`"),
        )
        .expect_get_property(Some(vec!["filter_state", "wasm.kuadrant.auth.identity.name"]))
        .returning(None)
        .expect_log(
            Some(LogLevel::Debug),
            Some("Adding data: `io.kuadrant` with entries: [\"bar\", \"foo\"]")
        )
        .expect_log(
            Some(LogLevel::Debug),
            Some("Dispatching gRPC call to authorino-cluster/envoy.service.auth.v3.Authorization.Check, timeout: 5s"),
        )
        .expect_grpc_call(
            Some("authorino-cluster"),
            Some("envoy.service.auth.v3.Authorization"),
            Some("Check"),
            Some(&[0, 0, 0, 0]),
            None,
            Some(5000),
        )
        .returning(Ok(42))
        .expect_log(
            Some(LogLevel::Debug),
            Some("gRPC call dispatched successfully, token_id: 42"),
        )
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();
    // The above request is validated as None because the protobuf Struct fields is a HashMap and so they ordering is not guaranteed.
    let grpc_response: [u8; 6] = [10, 0, 34, 0, 26, 0];
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
            Some("Getting property: `request.method`"),
        )
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(data::request::method::POST))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Selected blueprint some-name for hostname: cars.toystore.com"),
        )
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 pipeline built successfully"),
        )
        // retrieving properties for CheckRequest
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting map: `HttpRequestHeaders`"),
        )
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(None)
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `request.scheme`"),
        )
        .expect_get_property(Some(vec!["request", "scheme"]))
        .returning(Some(data::request::scheme::HTTP))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `request.path`"),
        )
        .expect_get_property(Some(vec!["request", "path"]))
        .returning(Some(data::request::path::ADMIN_TOY))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `request.protocol`"),
        )
        .expect_get_property(Some(vec!["request", "protocol"]))
        .returning(Some(data::request::protocol::HTTP_1_1))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `request.time`"),
        )
        .expect_get_property(Some(vec!["request", "time"]))
        .returning(Some(data::request::TIME))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `destination.address`"),
        )
        .expect_get_property(Some(vec!["destination", "address"]))
        .returning(Some(data::destination::ADDRESS))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `destination.port`"),
        )
        .expect_get_property(Some(vec!["destination", "port"]))
        .returning(Some(data::destination::port::P_8000))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `source.address`"),
        )
        .expect_get_property(Some(vec!["source", "address"]))
        .returning(Some(data::source::ADDRESS))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `source.port`"),
        )
        .expect_get_property(Some(vec!["source", "port"]))
        .returning(Some(data::source::port::P_45000))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Dispatching gRPC call to authorino-cluster/envoy.service.auth.v3.Authorization.Check, timeout: 5s"),
        )
        .expect_grpc_call(
            Some("authorino-cluster"),
            Some("envoy.service.auth.v3.Authorization"),
            Some("Check"),
            Some(&[0, 0, 0, 0]),
            None,
            Some(5000),
        )
        .returning(Ok(42))
        .expect_log(
            Some(LogLevel::Debug),
            Some("gRPC call dispatched successfully, token_id: 42"),
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
        .expect_log(
            Some(LogLevel::Debug),
            Some(format!("Getting gRPC response, size: {} bytes", grpc_response.len()).as_str()),
        )
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(&grpc_response))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Sending local reply, status code: 401"),
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
fn it_does_not_fold_auth_actions() {
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
            "auth": {
                "type": "auth",
                "endpoint": "authorino-cluster",
                "failureMode": "deny",
                "timeout": "5s"
            }
        },
        "actionSets": [
        {
            "name": "some-name",
            "routeRuleConditions": {
                "hostnames": ["*.com"]
            },
            "actions": [
            {
                "service": "auth",
                "scope": "auth-scope",
                "conditionalData" : []
            },
            {
                "service": "auth",
                "scope": "auth-scope",
                "conditionalData" : []
            }]
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
        .expect_log(
            Some(LogLevel::Debug),
            Some("#2 pipeline built successfully"),
        )
        // retrieving properties for CheckRequest
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting map: `HttpRequestHeaders`"),
        )
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(None)
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `request.method`"),
        )
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(data::request::method::GET))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `request.scheme`"),
        )
        .expect_get_property(Some(vec!["request", "scheme"]))
        .returning(Some(data::request::scheme::HTTP))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `request.path`"),
        )
        .expect_get_property(Some(vec!["request", "path"]))
        .returning(Some(data::request::path::ADMIN_TOY))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `request.protocol`"),
        )
        .expect_get_property(Some(vec!["request", "protocol"]))
        .returning(Some(data::request::protocol::HTTP_1_1))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `request.time`"),
        )
        .expect_get_property(Some(vec!["request", "time"]))
        .returning(Some(data::request::TIME))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `destination.address`"),
        )
        .expect_get_property(Some(vec!["destination", "address"]))
        .returning(Some(data::destination::ADDRESS))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `destination.port`"),
        )
        .expect_get_property(Some(vec!["destination", "port"]))
        .returning(Some(data::destination::port::P_8000))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `source.address`"),
        )
        .expect_get_property(Some(vec!["source", "address"]))
        .returning(Some(data::source::ADDRESS))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Getting property: `source.port`"),
        )
        .expect_get_property(Some(vec!["source", "port"]))
        .returning(Some(data::source::port::P_45000))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Dispatching gRPC call to authorino-cluster/envoy.service.auth.v3.Authorization.Check, timeout: 5s"),
        )
        .expect_grpc_call(
            Some("authorino-cluster"),
            Some("envoy.service.auth.v3.Authorization"),
            Some("Check"),
            Some(&[0, 0, 0, 0]),
            None,
            Some(5000),
        )
        .returning(Ok(42))
        .expect_log(
            Some(LogLevel::Debug),
            Some("gRPC call dispatched successfully, token_id: 42"),
        )
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    // TODO: response containing dynamic metadata
    // set_property is panicking with proxy-wasm-test-framework
    // because the `expect_set_property` is not yet implemented neither on original repo nor our fork
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
        .expect_log(
            Some(LogLevel::Debug),
            Some(format!("Getting gRPC response, size: {} bytes", grpc_response.len()).as_str()),
        )
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(&grpc_response))
        .expect_log(
            Some(LogLevel::Debug),
            Some("Dispatching gRPC call to authorino-cluster/envoy.service.auth.v3.Authorization.Check, timeout: 5s"),
        )
        .expect_grpc_call(
            Some("authorino-cluster"),
            Some("envoy.service.auth.v3.Authorization"),
            Some("Check"),
            Some(&[0, 0, 0, 0]),
            None,
            Some(5000),
        )
        .returning(Ok(43))
        .expect_log(
            Some(LogLevel::Debug),
            Some("gRPC call dispatched successfully, token_id: 43"),
        )
        .execute_and_expect(ReturnType::None)
        .unwrap();

    // TODO: response containing dynamic metadata
    // set_property is panicking with proxy-wasm-test-framework
    // because the `expect_set_property` is not yet implemented neither on original repo nor our fork
    // let grpc_response: [u8; 41] = [
    //     10, 0, 34, 35, 10, 33, 10, 8, 105, 100, 101, 110, 116, 105, 116, 121, 18, 21, 42, 19, 10,
    //     17, 10, 6, 117, 115, 101, 114, 105, 100, 18, 7, 26, 5, 97, 108, 105, 99, 101, 26, 0,
    // ];
    let grpc_response: [u8; 6] = [10, 0, 34, 0, 26, 0];
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

    module
        .call_proxy_on_response_headers(http_context, 0, false)
        .expect_log(Some(LogLevel::Debug), Some("#2 on_http_response_headers"))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();
}
