use crate::util::common::{auth_check_request_cel, json_escape_cel, wasm_module, LOG_LEVEL};
use crate::util::data;
use proxy_wasm_test_framework::tester;
use proxy_wasm_test_framework::types::{
    Action, BufferType, LogLevel, MapType, MetricType, ReturnType,
};
use serial_test::serial;

pub mod util;

fn config() -> String {
    let auth_msg = auth_check_request_cel("authconfig-A");
    r#"{
    "services": {
        "authorino": {
            "type": "dynamic",
            "endpoint": "authorino-cluster",
            "failureMode": "deny",
            "timeout": "5s",
            "grpcService": "envoy.service.auth.v3.Authorization",
            "grpcMethod": "Check"
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
            "type": "grpc",
            "var": "auth_response",
            "service": "authorino",
            "predicate": "true",
            "terminal": false,
            "label": "auth",
            "messageBuilder": "__AUTH_MSG__",
            "onReply": [
                {
                    "type": "deny",
                    "predicate": "has(auth_response.denied_response)",
                    "terminal": true,
                    "denyWith": "DenyResponse{status: (auth_response.denied_response.status.code != 0) ? uint(auth_response.denied_response.status.code) : 403u, headers: auth_response.denied_response.headers, body: auth_response.denied_response.body}"
                },
                {
                    "type": "fail",
                    "predicate": "has(auth_response.ok_response) && (auth_response.ok_response.response_headers_to_add.size() > 0 || auth_response.ok_response.headers_to_remove.size() > 0 || auth_response.ok_response.query_parameters_to_set.size() > 0 || auth_response.ok_response.query_parameters_to_remove.size() > 0)",
                    "terminal": true,
                    "logMessage": "Unsupported field in OkHttpResponse"
                },
                {
                    "type": "store",
                    "predicate": "has(auth_response.ok_response) && has(auth_response.dynamic_metadata)",
                    "terminal": false,
                    "path": "auth",
                    "value": "auth_response.dynamic_metadata",
                    "exportToHost": true
                },
                {
                    "type": "headers",
                    "predicate": "has(auth_response.ok_response)",
                    "terminal": false,
                    "target": "request",
                    "headers": "auth_response.ok_response.headers"
                },
                {
                    "type": "fail",
                    "predicate": "!has(auth_response.denied_response) && !has(auth_response.ok_response)",
                    "terminal": true,
                    "logMessage": "Auth response contained no http_response from auth_response"
                }
            ]
        }]
    }]
}"#.replace("__AUTH_MSG__", &json_escape_cel(&auth_msg))
}

#[test]
#[serial]
fn it_auths() {
    let cfg = config();
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
        .expect_get_property(Some(vec!["request", "url_path"]))
        .returning(Some(data::request::path::ADMIN_TOY))
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(data::request::method::POST))
        // retrieving properties for CheckRequest
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(Some(vec![(
            "x-request-id",
            "e1fc297a-a8a3-4360-8f41-af57b4a861e1",
        )]))
        .expect_get_property(Some(vec!["request", "time"]))
        .returning(Some(data::request::TIME))
        .expect_get_property(Some(vec!["request", "scheme"]))
        .returning(Some(data::request::scheme::HTTP))
        .expect_get_property(Some(vec!["request", "path"]))
        .returning(Some(data::request::path::ADMIN_TOY))
        .expect_get_property(Some(vec!["request", "protocol"]))
        .returning(Some(data::request::protocol::HTTP_1_1))
        .expect_get_property(Some(vec!["destination", "address"]))
        .returning(Some(data::destination::ADDRESS))
        .expect_get_property(Some(vec!["destination", "port"]))
        .returning(Some(data::destination::port::P_8000))
        .expect_get_property(Some(vec!["source", "address"]))
        .returning(Some(data::source::ADDRESS))
        .expect_get_property(Some(vec!["source", "port"]))
        .returning(Some(data::source::port::P_45000))
        .expect_increment_metric(Some(2), Some(1))
        .expect_grpc_call(
            Some("authorino-cluster"),
            Some("envoy.service.auth.v3.Authorization"),
            Some("Check"),
            None,
            None,
            Some(5000),
        )
        .returning(Ok(42))
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    let grpc_response = data::auth_response::WITH_METADATA_USERID_ALICE;
    module
        .call_proxy_on_grpc_receive(http_context, 42, grpc_response.len() as i32)
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(grpc_response))
        .expect_set_property(
            Some(vec!["kuadrant.auth"]),
            Some(data::auth_response::METADATA_STRUCT_USERID_ALICE),
        )
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
fn it_passes_request_data() {
    let auth_msg = r#"envoy.service.auth.v3.CheckRequest {
        attributes: envoy.service.auth.v3.AttributeContext {
            request: envoy.service.auth.v3.AttributeContext.Request {
                time: request.time,
                http: envoy.service.auth.v3.AttributeContext.HttpRequest {
                    host: request.host,
                    method: request.method,
                    scheme: request.scheme,
                    path: request.path,
                    protocol: request.protocol,
                    headers: request.headers
                }
            },
            destination: envoy.service.auth.v3.AttributeContext.Peer {
                address: envoy.config.core.v3.Address {
                    socket_address: envoy.config.core.v3.SocketAddress {
                        address: destination.address,
                        port_value: uint(destination.port)
                    }
                }
            },
            source: envoy.service.auth.v3.AttributeContext.Peer {
                address: envoy.config.core.v3.Address {
                    socket_address: envoy.config.core.v3.SocketAddress {
                        address: source.address,
                        port_value: uint(source.port)
                    }
                }
            },
            context_extensions: {"host": "authconfig-A"},
            metadata_context: envoy.config.core.v3.Metadata{
                filter_metadata: {
                    "io.kuadrant": google.protobuf.Struct{
                        fields: {
                            "bar": google.protobuf.Value{
                                struct_value: google.protobuf.Struct{
                                    fields: {
                                        "cel_expr": google.protobuf.Value{
                                            string_value: "auth.identity.name"
                                        }
                                    }
                                }
                            },
                            "foo": google.protobuf.Value{
                                struct_value: google.protobuf.Struct{
                                    fields: {
                                        "cel_expr": google.protobuf.Value{
                                            string_value: "string(2 + 3)"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }"#;
    let cfg = r#"{
        "services": {
            "authorino": {
                "type": "dynamic",
                "endpoint": "authorino-cluster",
                "failureMode": "deny",
                "timeout": "5s",
                "grpcService": "envoy.service.auth.v3.Authorization",
                "grpcMethod": "Check"
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
                "type": "grpc",
                "var": "auth_response",
                "service": "authorino",
                "predicate": "true",
                "terminal": false,
                "label": "auth",
                "messageBuilder": "__AUTH_MSG__",
                "onReply": [
                    {
                        "type": "deny",
                        "predicate": "has(auth_response.denied_response)",
                        "terminal": true,
                        "denyWith": "DenyResponse{status: (auth_response.denied_response.status.code != 0) ? uint(auth_response.denied_response.status.code) : 403u, headers: auth_response.denied_response.headers, body: auth_response.denied_response.body}"
                    },
                    {
                        "type": "fail",
                        "predicate": "has(auth_response.ok_response) && (auth_response.ok_response.response_headers_to_add.size() > 0 || auth_response.ok_response.headers_to_remove.size() > 0 || auth_response.ok_response.query_parameters_to_set.size() > 0 || auth_response.ok_response.query_parameters_to_remove.size() > 0)",
                        "terminal": true,
                        "logMessage": "Unsupported field in OkHttpResponse"
                    },
                    {
                        "type": "store",
                        "predicate": "has(auth_response.ok_response) && has(auth_response.dynamic_metadata)",
                        "terminal": false,
                        "path": "auth",
                        "value": "auth_response.dynamic_metadata",
                        "exportToHost": true
                    },
                    {
                        "type": "headers",
                        "predicate": "has(auth_response.ok_response)",
                        "terminal": false,
                        "target": "request",
                        "headers": "auth_response.ok_response.headers"
                    },
                    {
                        "type": "fail",
                        "predicate": "!has(auth_response.denied_response) && !has(auth_response.ok_response)",
                        "terminal": true,
                        "logMessage": "Auth response contained no http_response from auth_response"
                    }
                ]
            }]
        }]
    }"#.replace("__AUTH_MSG__", &json_escape_cel(auth_msg));

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
        .expect_get_property(Some(vec!["request", "url_path"]))
        .returning(Some(data::request::path::ADMIN_TOY))
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(data::request::method::POST))
        // retrieving properties for CheckRequest
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(None)
        .expect_get_property(Some(vec!["request", "time"]))
        .returning(Some(data::request::TIME))
        .expect_get_property(Some(vec!["request", "scheme"]))
        .returning(Some(data::request::scheme::HTTP))
        .expect_get_property(Some(vec!["request", "path"]))
        .returning(Some(data::request::path::ADMIN_TOY))
        .expect_get_property(Some(vec!["request", "protocol"]))
        .returning(Some(data::request::protocol::HTTP_1_1))
        .expect_get_property(Some(vec!["destination", "address"]))
        .returning(Some(data::destination::ADDRESS))
        .expect_get_property(Some(vec!["destination", "port"]))
        .returning(Some(data::destination::port::P_8000))
        .expect_get_property(Some(vec!["source", "address"]))
        .returning(Some(data::source::ADDRESS))
        .expect_get_property(Some(vec!["source", "port"]))
        .returning(Some(data::source::port::P_45000))
        .expect_increment_metric(Some(2), Some(1))
        .expect_grpc_call(
            Some("authorino-cluster"),
            Some("envoy.service.auth.v3.Authorization"),
            Some("Check"),
            None,
            None,
            Some(5000),
        )
        .returning(Ok(42))
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();
    // The above request is validated as None because the protobuf Struct fields is a HashMap and so they ordering is not guaranteed.
    let grpc_response: [u8; 4] = [10, 0, 26, 0];
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

#[test]
#[serial]
fn it_denies() {
    let cfg = config();
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
        .expect_get_property(Some(vec!["request", "url_path"]))
        .returning(Some(data::request::path::ADMIN_TOY))
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(data::request::method::POST))
        // retrieving properties for CheckRequest
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(None)
        .expect_get_property(Some(vec!["request", "time"]))
        .returning(Some(data::request::TIME))
        .expect_get_property(Some(vec!["request", "scheme"]))
        .returning(Some(data::request::scheme::HTTP))
        .expect_get_property(Some(vec!["request", "path"]))
        .returning(Some(data::request::path::ADMIN_TOY))
        .expect_get_property(Some(vec!["request", "protocol"]))
        .returning(Some(data::request::protocol::HTTP_1_1))
        .expect_get_property(Some(vec!["destination", "address"]))
        .returning(Some(data::destination::ADDRESS))
        .expect_get_property(Some(vec!["destination", "port"]))
        .returning(Some(data::destination::port::P_8000))
        .expect_get_property(Some(vec!["source", "address"]))
        .returning(Some(data::source::ADDRESS))
        .expect_get_property(Some(vec!["source", "port"]))
        .returning(Some(data::source::port::P_45000))
        .expect_increment_metric(Some(2), Some(1))
        .expect_grpc_call(
            Some("authorino-cluster"),
            Some("envoy.service.auth.v3.Authorization"),
            Some("Check"),
            None,
            None,
            Some(5000),
        )
        .returning(Ok(42))
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
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(&grpc_response))
        .expect_increment_metric(Some(5), Some(1))
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
    let auth_msg = auth_check_request_cel("auth-scope");
    let cfg = r#"{
        "services": {
            "authorino": {
                "type": "dynamic",
                "endpoint": "authorino-cluster",
                "failureMode": "deny",
                "timeout": "5s",
                "grpcService": "envoy.service.auth.v3.Authorization",
                "grpcMethod": "Check"
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
                "type": "grpc",
                "execution": "sequential",
                "var": "auth_response",
                "service": "authorino",
                "predicate": "true",
                "terminal": false,
                "label": "auth",
                "messageBuilder": "__AUTH_MSG__",
                "onReply": [
                    {
                        "type": "deny",
                        "predicate": "has(auth_response.denied_response)",
                        "terminal": true,
                        "denyWith": "DenyResponse{status: (auth_response.denied_response.status.code != 0) ? uint(auth_response.denied_response.status.code) : 403u, headers: auth_response.denied_response.headers, body: auth_response.denied_response.body}"
                    },
                    {
                        "type": "fail",
                        "predicate": "has(auth_response.ok_response) && (auth_response.ok_response.response_headers_to_add.size() > 0 || auth_response.ok_response.headers_to_remove.size() > 0 || auth_response.ok_response.query_parameters_to_set.size() > 0 || auth_response.ok_response.query_parameters_to_remove.size() > 0)",
                        "terminal": true,
                        "logMessage": "Unsupported field in OkHttpResponse"
                    },
                    {
                        "type": "store",
                        "predicate": "has(auth_response.ok_response) && has(auth_response.dynamic_metadata)",
                        "terminal": false,
                        "path": "auth",
                        "value": "auth_response.dynamic_metadata",
                        "exportToHost": true
                    },
                    {
                        "type": "headers",
                        "predicate": "has(auth_response.ok_response)",
                        "terminal": false,
                        "target": "request",
                        "headers": "auth_response.ok_response.headers"
                    },
                    {
                        "type": "fail",
                        "predicate": "!has(auth_response.denied_response) && !has(auth_response.ok_response)",
                        "terminal": true,
                        "logMessage": "Auth response contained no http_response from auth_response"
                    }
                ]
            },
            {
                "type": "grpc",
                "execution": "sequential",
                "var": "auth_response",
                "service": "authorino",
                "predicate": "true",
                "terminal": false,
                "label": "auth",
                "messageBuilder": "__AUTH_MSG__",
                "onReply": [
                    {
                        "type": "deny",
                        "predicate": "has(auth_response.denied_response)",
                        "terminal": true,
                        "denyWith": "DenyResponse{status: (auth_response.denied_response.status.code != 0) ? uint(auth_response.denied_response.status.code) : 403u, headers: auth_response.denied_response.headers, body: auth_response.denied_response.body}"
                    },
                    {
                        "type": "fail",
                        "predicate": "has(auth_response.ok_response) && (auth_response.ok_response.response_headers_to_add.size() > 0 || auth_response.ok_response.headers_to_remove.size() > 0 || auth_response.ok_response.query_parameters_to_set.size() > 0 || auth_response.ok_response.query_parameters_to_remove.size() > 0)",
                        "terminal": true,
                        "logMessage": "Unsupported field in OkHttpResponse"
                    },
                    {
                        "type": "store",
                        "predicate": "has(auth_response.ok_response) && has(auth_response.dynamic_metadata)",
                        "terminal": false,
                        "path": "auth",
                        "value": "auth_response.dynamic_metadata",
                        "exportToHost": true
                    },
                    {
                        "type": "headers",
                        "predicate": "has(auth_response.ok_response)",
                        "terminal": false,
                        "target": "request",
                        "headers": "auth_response.ok_response.headers"
                    },
                    {
                        "type": "fail",
                        "predicate": "!has(auth_response.denied_response) && !has(auth_response.ok_response)",
                        "terminal": true,
                        "logMessage": "Auth response contained no http_response from auth_response"
                    }
                ]
            }]
        }]
    }"#
    .replace("__AUTH_MSG__", &json_escape_cel(&auth_msg));

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
        // retrieving properties for CheckRequest
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(None)
        .expect_get_property(Some(vec!["request", "time"]))
        .returning(Some(data::request::TIME))
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(data::request::method::GET))
        .expect_get_property(Some(vec!["request", "scheme"]))
        .returning(Some(data::request::scheme::HTTP))
        .expect_get_property(Some(vec!["request", "path"]))
        .returning(Some(data::request::path::ADMIN_TOY))
        .expect_get_property(Some(vec!["request", "protocol"]))
        .returning(Some(data::request::protocol::HTTP_1_1))
        .expect_get_property(Some(vec!["destination", "address"]))
        .returning(Some(data::destination::ADDRESS))
        .expect_get_property(Some(vec!["destination", "port"]))
        .returning(Some(data::destination::port::P_8000))
        .expect_get_property(Some(vec!["source", "address"]))
        .returning(Some(data::source::ADDRESS))
        .expect_get_property(Some(vec!["source", "port"]))
        .returning(Some(data::source::port::P_45000))
        .expect_increment_metric(Some(2), Some(1))
        .expect_grpc_call(
            Some("authorino-cluster"),
            Some("envoy.service.auth.v3.Authorization"),
            Some("Check"),
            None,
            None,
            Some(5000),
        )
        .returning(Ok(42))
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    let grpc_response: [u8; 4] = [10, 0, 26, 0];
    module
        .call_proxy_on_grpc_receive(http_context, 42, grpc_response.len() as i32)
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(&grpc_response))
        .expect_grpc_call(
            Some("authorino-cluster"),
            Some("envoy.service.auth.v3.Authorization"),
            Some("Check"),
            None,
            None,
            Some(5000),
        )
        .returning(Ok(43))
        .execute_and_expect(ReturnType::None)
        .unwrap();

    let grpc_response: [u8; 4] = [10, 0, 26, 0];
    module
        .call_proxy_on_grpc_receive(http_context, 43, grpc_response.len() as i32)
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

#[test]
#[serial]
fn it_replaces_headers() {
    let cfg = config();
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
        .expect_get_property(Some(vec!["request", "url_path"]))
        .returning(Some(data::request::path::ADMIN_TOY))
        .expect_get_property(Some(vec!["request", "method"]))
        .returning(Some(data::request::method::POST))
        // retrieving properties for CheckRequest - includes initial Authorization header
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(Some(vec![
            ("x-request-id", "e1fc297a-a8a3-4360-8f41-af57b4a861e1"),
            ("authorization", "Bearer initial-token"),
        ]))
        .expect_get_property(Some(vec!["request", "time"]))
        .returning(Some(data::request::TIME))
        .expect_get_property(Some(vec!["request", "scheme"]))
        .returning(Some(data::request::scheme::HTTP))
        .expect_get_property(Some(vec!["request", "path"]))
        .returning(Some(data::request::path::ADMIN_TOY))
        .expect_get_property(Some(vec!["request", "protocol"]))
        .returning(Some(data::request::protocol::HTTP_1_1))
        .expect_get_property(Some(vec!["destination", "address"]))
        .returning(Some(data::destination::ADDRESS))
        .expect_get_property(Some(vec!["destination", "port"]))
        .returning(Some(data::destination::port::P_8000))
        .expect_get_property(Some(vec!["source", "address"]))
        .returning(Some(data::source::ADDRESS))
        .expect_get_property(Some(vec!["source", "port"]))
        .returning(Some(data::source::port::P_45000))
        .expect_increment_metric(Some(2), Some(1))
        .expect_grpc_call(
            Some("authorino-cluster"),
            Some("envoy.service.auth.v3.Authorization"),
            Some("Check"),
            None,
            None,
            Some(5000),
        )
        .returning(Ok(42))
        .execute_and_expect(ReturnType::Action(Action::Pause))
        .unwrap();

    let grpc_response = data::auth_response::WITH_AUTHORIZATION_HEADER;
    module
        .call_proxy_on_grpc_receive(http_context, 42, grpc_response.len() as i32)
        .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
        .returning(Some(grpc_response))
        .expect_set_header_map_pairs(
            Some(MapType::HttpRequestHeaders),
            Some(vec![
                ("x-request-id", "e1fc297a-a8a3-4360-8f41-af57b4a861e1"),
                ("authorization", "Bearer replaced-token"),
            ]),
        )
        .execute_and_expect(ReturnType::None)
        .unwrap();

    module
        .call_proxy_on_response_headers(http_context, 0, false)
        .expect_increment_metric(Some(4), Some(1))
        .execute_and_expect(ReturnType::Action(Action::Continue))
        .unwrap();
}
