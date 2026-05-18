use super::{
    Action, ConditionalData, DataItem, DataType, DenyOperation, FailOperation, GrpcOperation,
    HeadersOperation, HeadersTarget, Operation, TypedAction,
};

fn escape_cel_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

fn join_predicates(predicates: &[String], op: &str) -> String {
    match predicates.len() {
        0 => "true".to_string(),
        1 => predicates[0].clone(),
        _ => predicates
            .iter()
            .map(|p| format!("({})", p))
            .collect::<Vec<_>>()
            .join(&format!(" {} ", op)),
    }
}

fn build_action_predicate(action_predicates: &[String]) -> String {
    join_predicates(action_predicates, "&&")
}

pub(super) mod ratelimit {
    use super::*;

    const RATELIMIT_KNOWN_ATTRS: [&str; 2] = ["ratelimit.domain", "ratelimit.hits_addend"];

    fn is_ratelimit_known_attr(item: &DataItem) -> bool {
        let key = match &item.item {
            DataType::Static(s) => s.key.as_str(),
            DataType::Expression(e) => e.key.as_str(),
        };
        RATELIMIT_KNOWN_ATTRS.contains(&key)
    }

    fn find_ratelimit_known_attr_cel(
        conditional_data: &[ConditionalData],
        attr_key: &str,
    ) -> Option<String> {
        for cd in conditional_data {
            for item in &cd.data {
                match &item.item {
                    DataType::Static(s) if s.key == attr_key => {
                        return Some(format!(r#""{}""#, escape_cel_string(&s.value)));
                    }
                    DataType::Expression(e) if e.key == attr_key => {
                        return Some(e.value.clone());
                    }
                    _ => {}
                }
            }
        }
        None
    }

    fn build_ratelimit_descriptor_entry_cel(item: &DataItem) -> String {
        let (key, value_cel) = match &item.item {
            DataType::Static(s) => (
                s.key.as_str(),
                format!(r#""{}""#, escape_cel_string(&s.value)),
            ),
            DataType::Expression(e) => (e.key.as_str(), format!("string({})", e.value)),
        };

        format!(
            r#"envoy.extensions.common.ratelimit.v3.RateLimitDescriptor.Entry {{ key: "{}", value: {} }}"#,
            escape_cel_string(key),
            value_cel,
        )
    }

    fn build_ratelimit_entry_list_cel(cd: &ConditionalData) -> Option<String> {
        let entries: Vec<String> = cd
            .data
            .iter()
            .filter(|item| !is_ratelimit_known_attr(item))
            .map(build_ratelimit_descriptor_entry_cel)
            .collect();

        if entries.is_empty() {
            return None;
        }

        let entries_list = format!("[{}]", entries.join(", "));

        if cd.predicates.is_empty() {
            Some(entries_list)
        } else {
            let predicate_cel = join_predicates(&cd.predicates, "&&");
            Some(format!("(({}) ? {} : [])", predicate_cel, entries_list))
        }
    }

    fn build_ratelimit_descriptors_cel(conditional_data: &[ConditionalData]) -> Option<String> {
        let entry_parts: Vec<String> = conditional_data
            .iter()
            .filter_map(build_ratelimit_entry_list_cel)
            .collect();

        if entry_parts.is_empty() {
            return None;
        }

        let combined_entries = entry_parts.join(" + ");
        Some(format!(
            "envoy.extensions.common.ratelimit.v3.RateLimitDescriptor {{ entries: {} }}",
            combined_entries
        ))
    }

    fn build_ratelimit_message_builder(
        scope: &str,
        conditional_data: &[ConditionalData],
        request_data: &[((String, String), String)],
    ) -> String {
        let domain_cel = find_ratelimit_known_attr_cel(conditional_data, "ratelimit.domain")
            .unwrap_or_else(|| format!(r#""{}""#, escape_cel_string(scope)));

        let hits_addend_cel =
            find_ratelimit_known_attr_cel(conditional_data, "ratelimit.hits_addend")
                .unwrap_or_else(|| "1u".to_string());

        let mut descriptors = vec![];

        if let Some(desc) = build_ratelimit_descriptors_cel(conditional_data) {
            descriptors.push(desc);
        }

        if !request_data.is_empty() {
            let request_data_entries: Vec<String> = request_data
                .iter()
                .map(|((domain, field), value_expr)| {
                    let key = if domain.is_empty() || domain == "metrics.labels" {
                        field.clone()
                    } else {
                        format!("{}.{}", domain, field)
                    };
                    format!(
                        r#"envoy.extensions.common.ratelimit.v3.RateLimitDescriptor.Entry {{ key: "{}", value: string({}) }}"#,
                        escape_cel_string(&key),
                        value_expr
                    )
                })
                .collect();

            if !request_data_entries.is_empty() {
                descriptors.push(format!(
                    "envoy.extensions.common.ratelimit.v3.RateLimitDescriptor {{ entries: [{}] }}",
                    request_data_entries.join(", ")
                ));
            }
        }

        let descriptors_cel = if descriptors.is_empty() {
            "[]".to_string()
        } else {
            format!("[{}]", descriptors.join(", "))
        };

        format!(
            r#"envoy.service.ratelimit.v3.RateLimitRequest {{
    domain: {},
    hits_addend: {},
    descriptors: {}
}}"#,
            domain_cel, hits_addend_cel, descriptors_cel,
        )
    }

    fn build_descriptor_predicate(conditional_data: &[ConditionalData]) -> String {
        if conditional_data.is_empty() {
            return "true".to_string();
        }
        if conditional_data.iter().any(|cd| cd.predicates.is_empty()) {
            return "true".to_string();
        }

        let block_predicates: Vec<String> = conditional_data
            .iter()
            .map(|cd| {
                if cd.predicates.len() == 1 {
                    cd.predicates[0].clone()
                } else {
                    format!("({})", join_predicates(&cd.predicates, "&&"))
                }
            })
            .collect();

        if block_predicates.len() == 1 {
            block_predicates[0].clone()
        } else {
            block_predicates.join(" || ")
        }
    }

    fn build_ratelimit_predicate(
        action_predicates: &[String],
        conditional_data: &[ConditionalData],
    ) -> String {
        let action_pred = build_action_predicate(action_predicates);
        let conditional_pred = build_descriptor_predicate(conditional_data);

        if action_pred == "true" && conditional_pred == "true" {
            "true".to_string()
        } else if action_pred == "true" {
            conditional_pred
        } else if conditional_pred == "true" {
            action_pred
        } else {
            format!("({}) && ({})", action_pred, conditional_pred)
        }
    }

    fn build_ratelimit_on_reply(name: &str) -> Vec<TypedAction> {
        vec![
            TypedAction {
                predicate: format!("{}.overall_code == 2", name),
                terminal: true,
                operation: Operation::Deny(DenyOperation {
                    deny_with: format!(
                        r#"DenyResponse{{status: 429u, headers: {}.response_headers_to_add, body: "Too Many Requests\n"}}"#,
                        name
                    ),
                }),
            },
            TypedAction {
                predicate: format!("{}.overall_code == 1", name),
                terminal: false,
                operation: Operation::Headers(HeadersOperation {
                    target: HeadersTarget::Response,
                    headers: format!("{}.response_headers_to_add", name),
                }),
            },
            TypedAction {
                predicate: format!("{}.overall_code != 1 && {}.overall_code != 2", name, name),
                terminal: true,
                operation: Operation::Fail(FailOperation {
                    log_message: format!("Unknown rate limit response code from {}", name),
                }),
            },
        ]
    }

    #[deprecated(note = "temporary translation for legacy ratelimit configuration")]
    pub(crate) fn translate_legacy_ratelimit_to_typed(
        action: &Action,
        request_data: &[((String, String), String)],
    ) -> TypedAction {
        const RESPONSE_VAR: &str = "ratelimit_response";

        let message_builder =
            build_ratelimit_message_builder(&action.scope, &action.conditional_data, request_data);

        let predicate = build_ratelimit_predicate(&action.predicates, &action.conditional_data);

        let on_reply = build_ratelimit_on_reply(RESPONSE_VAR);

        TypedAction {
            predicate,
            terminal: false,
            operation: Operation::Grpc(GrpcOperation {
                var: RESPONSE_VAR.to_string(),
                service: action.service.clone(),
                message_builder,
                on_reply,
            }),
        }
    }

    #[cfg(test)]
    #[allow(deprecated)]
    mod tests {
        use crate::configuration::{ExpressionItem, StaticItem};

        use super::*;

        #[test]
        fn test_build_descriptor_predicate_empty() {
            let conditional_data = vec![];
            assert_eq!(build_descriptor_predicate(&conditional_data), "true");
        }

        #[test]
        fn test_build_descriptor_predicate_unconditional() {
            let conditional_data = vec![ConditionalData {
                predicates: vec![],
                data: vec![DataItem {
                    item: DataType::Static(StaticItem {
                        key: "limit".to_string(),
                        value: "10".to_string(),
                    }),
                }],
            }];
            assert_eq!(build_descriptor_predicate(&conditional_data), "true");
        }

        #[test]
        fn test_build_descriptor_predicate_single_predicate() {
            let conditional_data = vec![ConditionalData {
                predicates: vec!["auth.identity.user == 'alice'".to_string()],
                data: vec![],
            }];
            assert_eq!(
                build_descriptor_predicate(&conditional_data),
                "auth.identity.user == 'alice'"
            );
        }

        #[test]
        fn test_build_descriptor_predicate_multiple_predicates_single_block() {
            let conditional_data = vec![ConditionalData {
                predicates: vec![
                    "auth.identity.user == 'alice'".to_string(),
                    "request.method == 'POST'".to_string(),
                ],
                data: vec![],
            }];
            assert_eq!(
                build_descriptor_predicate(&conditional_data),
                "((auth.identity.user == 'alice') && (request.method == 'POST'))"
            );
        }

        #[test]
        fn test_build_descriptor_predicate_multiple_blocks() {
            let conditional_data = vec![
                ConditionalData {
                    predicates: vec!["auth.identity.role == 'admin'".to_string()],
                    data: vec![],
                },
                ConditionalData {
                    predicates: vec!["auth.identity.role == 'user'".to_string()],
                    data: vec![],
                },
            ];
            assert_eq!(
                build_descriptor_predicate(&conditional_data),
                "auth.identity.role == 'admin' || auth.identity.role == 'user'"
            );
        }

        #[test]
        fn test_build_descriptor_predicate_mixed_conditional_unconditional() {
            let conditional_data = vec![
                ConditionalData {
                    predicates: vec!["auth.identity.role == 'admin'".to_string()],
                    data: vec![],
                },
                ConditionalData {
                    predicates: vec![],
                    data: vec![],
                },
            ];
            assert_eq!(build_descriptor_predicate(&conditional_data), "true");
        }

        #[test]
        fn test_build_ratelimit_on_reply_structure() {
            let on_reply = build_ratelimit_on_reply("rl_response");

            assert_eq!(on_reply.len(), 3);

            assert_eq!(on_reply[0].predicate, "rl_response.overall_code == 2");
            assert!(on_reply[0].terminal);
            assert!(matches!(on_reply[0].operation, Operation::Deny(_)));

            assert_eq!(on_reply[1].predicate, "rl_response.overall_code == 1");
            assert!(!on_reply[1].terminal);
            assert!(matches!(on_reply[1].operation, Operation::Headers(_)));

            assert_eq!(
                on_reply[2].predicate,
                "rl_response.overall_code != 1 && rl_response.overall_code != 2"
            );
            assert!(on_reply[2].terminal);
            assert!(matches!(on_reply[2].operation, Operation::Fail(_)));
        }

        #[test]
        fn test_build_ratelimit_on_reply_deny_operation() {
            let on_reply = build_ratelimit_on_reply("test_var");

            assert!(matches!(&on_reply[0].operation,
                Operation::Deny(deny_op) if
                    deny_op.deny_with == r#"DenyResponse{status: 429u, headers: test_var.response_headers_to_add, body: "Too Many Requests\n"}"#
            ));
        }

        #[test]
        fn test_build_ratelimit_on_reply_headers_operation() {
            let on_reply = build_ratelimit_on_reply("my_rl");

            assert!(matches!(&on_reply[1].operation,
                Operation::Headers(headers_op) if
                    matches!(headers_op.target, HeadersTarget::Response) &&
                    headers_op.headers == "my_rl.response_headers_to_add"
            ));
        }

        #[test]
        fn test_build_ratelimit_on_reply_fail_operation() {
            let on_reply = build_ratelimit_on_reply("rate_limit");

            assert!(matches!(&on_reply[2].operation,
                Operation::Fail(fail_op) if
                    fail_op.log_message == "Unknown rate limit response code from rate_limit"
            ));
        }

        #[test]
        fn test_translate_legacy_ratelimit_simple() {
            let action = Action {
                service: "limitador".to_string(),
                scope: "my-ratelimit".to_string(),
                predicates: vec![],
                conditional_data: vec![],
                sources: vec![],
            };
            let request_data = vec![];

            let typed = translate_legacy_ratelimit_to_typed(&action, &request_data);

            assert_eq!(typed.predicate, "true");
            assert!(!typed.terminal);

            assert!(matches!(&typed.operation,
                Operation::Grpc(grpc_op) if
                    grpc_op.var == "ratelimit_response" &&
                    grpc_op.service == "limitador" &&
                    grpc_op.message_builder == r#"envoy.service.ratelimit.v3.RateLimitRequest {
    domain: "my-ratelimit",
    hits_addend: 1u,
    descriptors: []
}"# &&
                    grpc_op.on_reply.len() == 3
            ));
        }

        #[test]
        fn test_translate_legacy_ratelimit_with_conditional_data() {
            let action = Action {
                service: "limitador".to_string(),
                scope: "my-ratelimit".to_string(),
                predicates: vec![],
                conditional_data: vec![ConditionalData {
                    predicates: vec!["auth.identity.user == 'alice'".to_string()],
                    data: vec![DataItem {
                        item: DataType::Static(StaticItem {
                            key: "tier".to_string(),
                            value: "gold".to_string(),
                        }),
                    }],
                }],
                sources: vec![],
            };
            let request_data = vec![];

            let typed = translate_legacy_ratelimit_to_typed(&action, &request_data);

            assert_eq!(typed.predicate, "auth.identity.user == 'alice'");

            assert!(matches!(&typed.operation,
                Operation::Grpc(grpc_op) if
                    grpc_op.message_builder == r#"envoy.service.ratelimit.v3.RateLimitRequest {
    domain: "my-ratelimit",
    hits_addend: 1u,
    descriptors: [envoy.extensions.common.ratelimit.v3.RateLimitDescriptor { entries: ((auth.identity.user == 'alice') ? [envoy.extensions.common.ratelimit.v3.RateLimitDescriptor.Entry { key: "tier", value: "gold" }] : []) }]
}"#
            ));
        }

        #[test]
        fn test_translate_legacy_ratelimit_with_request_data() {
            let action = Action {
                service: "limitador".to_string(),
                scope: "default".to_string(),
                predicates: vec![],
                conditional_data: vec![],
                sources: vec![],
            };
            let request_data = vec![(
                ("".to_string(), "env".to_string()),
                r#""production""#.to_string(),
            )];

            let typed = translate_legacy_ratelimit_to_typed(&action, &request_data);

            assert!(matches!(&typed.operation,
                Operation::Grpc(grpc_op) if
                    grpc_op.message_builder == r#"envoy.service.ratelimit.v3.RateLimitRequest {
    domain: "default",
    hits_addend: 1u,
    descriptors: [envoy.extensions.common.ratelimit.v3.RateLimitDescriptor { entries: [envoy.extensions.common.ratelimit.v3.RateLimitDescriptor.Entry { key: "env", value: string("production") }] }]
}"#
            ));
        }

        #[test]
        fn test_translate_legacy_ratelimit_full() {
            let action = Action {
                service: "limitador".to_string(),
                scope: "rlp-full".to_string(),
                predicates: vec![],
                conditional_data: vec![
                    ConditionalData {
                        predicates: vec!["auth.identity.role == 'admin'".to_string()],
                        data: vec![DataItem {
                            item: DataType::Static(StaticItem {
                                key: "tier".to_string(),
                                value: "gold".to_string(),
                            }),
                        }],
                    },
                    ConditionalData {
                        predicates: vec![],
                        data: vec![DataItem {
                            item: DataType::Expression(ExpressionItem {
                                key: "method".to_string(),
                                value: "request.method".to_string(),
                            }),
                        }],
                    },
                ],
                sources: vec![],
            };
            let request_data = vec![(
                ("".to_string(), "env".to_string()),
                r#""production""#.to_string(),
            )];

            let typed = translate_legacy_ratelimit_to_typed(&action, &request_data);

            assert_eq!(typed.predicate, "true");

            assert!(matches!(&typed.operation,
                Operation::Grpc(grpc_op) if
                    grpc_op.message_builder == r#"envoy.service.ratelimit.v3.RateLimitRequest {
    domain: "rlp-full",
    hits_addend: 1u,
    descriptors: [envoy.extensions.common.ratelimit.v3.RateLimitDescriptor { entries: ((auth.identity.role == 'admin') ? [envoy.extensions.common.ratelimit.v3.RateLimitDescriptor.Entry { key: "tier", value: "gold" }] : []) + [envoy.extensions.common.ratelimit.v3.RateLimitDescriptor.Entry { key: "method", value: string(request.method) }] }, envoy.extensions.common.ratelimit.v3.RateLimitDescriptor { entries: [envoy.extensions.common.ratelimit.v3.RateLimitDescriptor.Entry { key: "env", value: string("production") }] }]
}"#
            ));
        }
    }
}

pub(super) mod auth {
    use super::*;
    use std::collections::HashMap;

    fn build_request_data_value_cel(field_expr: &str) -> String {
        if field_expr.contains("auth.") {
            format!(
                r#"google.protobuf.Value{{struct_value: google.protobuf.Struct{{fields: {{"cel_expr": google.protobuf.Value{{string_value: "{escaped_expr}"}}}}}}}}"#,
                escaped_expr = escape_cel_string(field_expr)
            )
        } else {
            format!(
                r#"google.protobuf.Value{{string_value: string({expr})}}"#,
                expr = field_expr
            )
        }
    }

    fn build_metadata_context_cel(request_data: &[((String, String), String)]) -> String {
        if request_data.is_empty() {
            return "envoy.config.core.v3.Metadata{}".to_string();
        }

        let mut by_domain: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for ((domain, field), expr) in request_data {
            let key = if domain.is_empty() {
                "io.kuadrant".to_string()
            } else {
                format!("io.kuadrant.{}", domain)
            };
            by_domain
                .entry(key)
                .or_default()
                .push((field.clone(), expr.clone()));
        }

        let mut domain_entries: Vec<(String, Vec<(String, String)>)> =
            by_domain.into_iter().collect();
        domain_entries.sort_by(|a, b| a.0.cmp(&b.0));

        let filter_metadata_entries: Vec<String> = domain_entries
            .into_iter()
            .map(|(domain, entries)| {
                let field_entries: Vec<String> = entries
                    .iter()
                    .map(|(field, expr)| {
                        format!(
                            r#""{field}": {value}"#,
                            field = escape_cel_string(field),
                            value = build_request_data_value_cel(expr)
                        )
                    })
                    .collect();

                format!(
                    r#""{domain}": google.protobuf.Struct{{fields: {{{fields}}}}}"#,
                    domain = escape_cel_string(&domain),
                    fields = field_entries.join(", ")
                )
            })
            .collect();

        format!(
            "envoy.config.core.v3.Metadata{{filter_metadata: {{{}}}}}",
            filter_metadata_entries.join(", ")
        )
    }

    fn build_auth_message_builder(
        scope: &str,
        request_data: &[((String, String), String)],
    ) -> String {
        let metadata_context = build_metadata_context_cel(request_data);

        format!(
            r#"envoy.service.auth.v3.CheckRequest {{
  attributes: envoy.service.auth.v3.AttributeContext {{
    request: envoy.service.auth.v3.AttributeContext.Request {{
      time: request.time,
      http: envoy.service.auth.v3.AttributeContext.HttpRequest {{
        host: request.host,
        method: request.method,
        scheme: request.scheme,
        path: request.path,
        protocol: request.protocol,
        headers: request.headers
      }}
    }},
    destination: envoy.service.auth.v3.AttributeContext.Peer {{
      address: envoy.config.core.v3.Address {{
        socket_address: envoy.config.core.v3.SocketAddress {{
          address: destination.address,
          port_value: uint(destination.port)
        }}
      }}
    }},
    source: envoy.service.auth.v3.AttributeContext.Peer {{
      address: envoy.config.core.v3.Address {{
        socket_address: envoy.config.core.v3.SocketAddress {{
          address: source.address,
          port_value: uint(source.port)
        }}
      }}
    }},
    context_extensions: {{"host": "{}"}},
    metadata_context: {}
  }}
}}"#,
            escape_cel_string(scope),
            metadata_context
        )
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_build_auth_message_builder_basic() {
            let message = build_auth_message_builder("test-scope", &[]);

            let expected = r#"envoy.service.auth.v3.CheckRequest {
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
    context_extensions: {"host": "test-scope"},
    metadata_context: envoy.config.core.v3.Metadata{}
  }
}"#;
            assert_eq!(message, expected);
        }

        #[test]
        fn test_build_auth_message_builder_escapes_scope() {
            let message = build_auth_message_builder("test\"scope", &[]);

            let expected = r#"envoy.service.auth.v3.CheckRequest {
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
    context_extensions: {"host": "test\"scope"},
    metadata_context: envoy.config.core.v3.Metadata{}
  }
}"#;
            assert_eq!(message, expected);
        }

        #[test]
        fn test_build_metadata_context_cel_empty() {
            let cel = build_metadata_context_cel(&[]);
            assert_eq!(cel, "envoy.config.core.v3.Metadata{}");
        }

        #[test]
        fn test_build_metadata_context_cel_single_field() {
            let request_data = vec![(
                ("".to_string(), "userid".to_string()),
                "auth.identity.userid".to_string(),
            )];
            let cel = build_metadata_context_cel(&request_data);

            let expected = r#"envoy.config.core.v3.Metadata{filter_metadata: {"io.kuadrant": google.protobuf.Struct{fields: {"userid": google.protobuf.Value{struct_value: google.protobuf.Struct{fields: {"cel_expr": google.protobuf.Value{string_value: "auth.identity.userid"}}}}}}}}"#;
            assert_eq!(cel, expected);
        }

        #[test]
        fn test_build_metadata_context_cel_multiple_domains() {
            let request_data = vec![
                (
                    ("".to_string(), "userid".to_string()),
                    "auth.identity.userid".to_string(),
                ),
                (
                    ("custom".to_string(), "role".to_string()),
                    "auth.identity.role".to_string(),
                ),
            ];
            let cel = build_metadata_context_cel(&request_data);

            let expected = r#"envoy.config.core.v3.Metadata{filter_metadata: {"io.kuadrant": google.protobuf.Struct{fields: {"userid": google.protobuf.Value{struct_value: google.protobuf.Struct{fields: {"cel_expr": google.protobuf.Value{string_value: "auth.identity.userid"}}}}}}, "io.kuadrant.custom": google.protobuf.Struct{fields: {"role": google.protobuf.Value{struct_value: google.protobuf.Struct{fields: {"cel_expr": google.protobuf.Value{string_value: "auth.identity.role"}}}}}}}}"#;
            assert_eq!(cel, expected);
        }

        #[test]
        fn test_build_auth_message_builder_with_request_data() {
            let request_data = vec![(
                ("".to_string(), "userid".to_string()),
                "auth.identity.userid".to_string(),
            )];
            let message = build_auth_message_builder("my-scope", &request_data);

            let expected = r#"envoy.service.auth.v3.CheckRequest {
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
    context_extensions: {"host": "my-scope"},
    metadata_context: envoy.config.core.v3.Metadata{filter_metadata: {"io.kuadrant": google.protobuf.Struct{fields: {"userid": google.protobuf.Value{struct_value: google.protobuf.Struct{fields: {"cel_expr": google.protobuf.Value{string_value: "auth.identity.userid"}}}}}}}}
  }
}"#;
            assert_eq!(message, expected);
        }
    }
}
