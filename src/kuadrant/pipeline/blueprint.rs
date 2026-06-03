#[allow(deprecated)]
use crate::configuration::{
    self, translate_legacy_auth_to_typed, translate_legacy_ratelimit_to_typed,
    translate_legacy_report_to_typed,
};
use crate::data::{cel::Predicate, Expression};
use crate::kuadrant::pipeline::tasks::{
    DynamicTask, ExportTracesTask, FailureModeTask, HeaderOperation, HeadersType,
    ModifyHeadersTask, Task, TeardownAction, TokenUsageTask, TracingDecoratorTask,
};
use crate::kuadrant::ReqRespCtx;
use crate::services::ServiceInstance;
use cel::ParseErrors;
use std::collections::HashMap;
use std::fmt::Display;
use std::rc::Rc;

pub type RequestData = ((String, String), Expression);

pub(crate) struct Blueprint {
    pub name: String,
    pub route_predicates: Vec<Predicate>,
    pub actions: Vec<Action>,
}

#[derive(Clone)]
pub(crate) struct Action {
    pub id: String,
    pub predicate: Predicate,
    pub terminal: bool,
    pub operation: Operation,
    pub dependencies: Vec<String>,
    pub sources: Vec<String>,
    pub is_guard: bool,
}

#[derive(Clone)]
pub(crate) enum Operation {
    Grpc {
        service: ServiceInstance,
        var: String,
        message_builder: Expression,
        on_reply: Vec<Action>,
        label: String,
    },
    Deny {
        deny_with: Expression,
    },
    Headers {
        target: HeadersType,
        headers: Expression,
    },
    Store {
        path: String,
        expression: Expression,
        export_to_host: bool,
    },
    Fail {
        log_message: String,
    },
}

impl Action {
    pub fn collect_body_values(&self, request_data: &[RequestData]) -> Vec<String> {
        use std::collections::HashSet;

        let mut fields = HashSet::new();

        fields.extend(self.predicate.response_body_values().iter().cloned());

        fields.extend(
            request_data
                .iter()
                .flat_map(|(_, expr)| expr.response_body_values().iter().cloned()),
        );

        match &self.operation {
            Operation::Grpc {
                message_builder,
                on_reply,
                ..
            } => {
                fields.extend(message_builder.response_body_values().iter().cloned());
                fields.extend(on_reply.iter().flat_map(|action| {
                    let mut reply_fields = Vec::new();
                    reply_fields.extend(action.predicate.response_body_values().iter().cloned());
                    match &action.operation {
                        Operation::Grpc {
                            message_builder,
                            on_reply: nested_reply,
                            ..
                        } => {
                            reply_fields
                                .extend(message_builder.response_body_values().iter().cloned());
                            reply_fields.extend(
                                nested_reply
                                    .iter()
                                    .flat_map(|nested| nested.collect_body_values(&[])),
                            );
                        }
                        Operation::Deny { deny_with } => {
                            reply_fields.extend(deny_with.response_body_values().iter().cloned());
                        }
                        Operation::Headers { headers, .. } => {
                            reply_fields.extend(headers.response_body_values().iter().cloned());
                        }
                        Operation::Store { expression, .. } => {
                            reply_fields.extend(expression.response_body_values().iter().cloned());
                        }
                        Operation::Fail { .. } => {}
                    }
                    reply_fields
                }));
            }
            Operation::Deny { deny_with } => {
                fields.extend(deny_with.response_body_values().iter().cloned());
            }
            Operation::Headers { headers, .. } => {
                fields.extend(headers.response_body_values().iter().cloned());
            }
            Operation::Store { expression, .. } => {
                fields.extend(expression.response_body_values().iter().cloned());
            }
            Operation::Fail { .. } => {}
        }

        fields.into_iter().collect()
    }
}

#[derive(Debug)]
pub enum CompileError {
    InvalidRoutePredicate { action_set: String, error: String },
    InvalidActionPredicate { service: String, error: String },
    InvalidDataExpression(String),
    UnknownService(String),
    ServiceCreationFailed(String),
}

impl From<ParseErrors> for CompileError {
    fn from(e: ParseErrors) -> Self {
        CompileError::InvalidDataExpression(e.to_string())
    }
}

impl Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::InvalidRoutePredicate { action_set, error } => {
                write!(f, "Invalid route predicate on {}: {}", action_set, error)
            }
            CompileError::InvalidActionPredicate { service, error } => {
                write!(f, "Invalid action predicate on {}: {}", service, error)
            }
            CompileError::InvalidDataExpression(msg) => {
                write!(f, "Invalid data expression: {}", msg)
            }
            CompileError::UnknownService(srv) => write!(f, "Unknown service: {}", srv),
            CompileError::ServiceCreationFailed(srv) => {
                write!(f, "Service creation failed: {}", srv)
            }
        }
    }
}

impl Blueprint {
    pub fn compile(
        config: &configuration::ActionSet,
        services: &HashMap<String, ServiceInstance>,
        request_data: &[RequestData],
    ) -> Result<Self, CompileError> {
        let route_predicates: Vec<Predicate> = config
            .route_rule_conditions
            .predicates
            .iter()
            .map(|p| Predicate::new(p))
            .collect::<Result<_, _>>()
            .map_err(|e| CompileError::InvalidRoutePredicate {
                action_set: config.name.clone(),
                error: e.to_string(),
            })?;

        let actions: Vec<Action> = config
            .actions
            .iter()
            .enumerate()
            .map(|(i, action_config)| {
                let id = i.to_string();
                let dependencies = if i > 0 {
                    vec![(i - 1).to_string()]
                } else {
                    vec![]
                };
                match action_config {
                    configuration::ActionConfig::Legacy(action) => {
                        let legacy_request_data: Vec<((String, String), String)> = request_data
                            .iter()
                            .map(|(key, expr)| (key.clone(), expr.source().to_string()))
                            .collect();
                        Action::compile(action, services, id, dependencies, &legacy_request_data)
                    }
                    configuration::ActionConfig::Typed(typed) => {
                        Action::compile_typed(typed, services, id, dependencies)
                    }
                }
            })
            .collect::<Result<_, _>>()?;

        Ok(Self {
            name: config.name.clone(),
            route_predicates,
            actions,
        })
    }
}

type TaskList = Vec<Box<dyn Task>>;
type TeardownList = Vec<Box<dyn TeardownAction>>;

impl Blueprint {
    pub fn to_tasks(
        &self,
        ctx: &mut ReqRespCtx,
        request_data: &[RequestData],
    ) -> (TaskList, TeardownList) {
        let mut tasks: TaskList = Vec::new();
        let mut teardown_tasks: TeardownList = Vec::new();

        let tracing_enabled = self.actions.iter().any(|action| {
            matches!(
                &action.operation,
                Operation::Grpc { service, .. } if matches!(service, ServiceInstance::Tracing(Some(_)))
            )
        });

        for action in &self.actions {
            match &action.operation {
                Operation::Grpc {
                    service,
                    var,
                    message_builder,
                    on_reply,
                    label,
                } => {
                    let abort_on_failure =
                        service.failure_mode() == configuration::FailureMode::Deny;

                    match service {
                        ServiceInstance::Tracing(tracing_service) => {
                            ctx.set_public_tracker_id(var.clone());
                            tasks.push(Box::new(ModifyHeadersTask::new(
                                HeaderOperation::Append(
                                    vec![(var.clone(), ctx.request_id().to_string())].into(),
                                ),
                                HeadersType::HttpResponseHeaders,
                            )));
                            if let Some(service) = tracing_service {
                                teardown_tasks
                                    .push(Box::new(ExportTracesTask::new(ctx, service.clone())));
                            }
                        }
                        ServiceInstance::Dynamic(dynamic_service)
                        | ServiceInstance::Auth(dynamic_service)
                        | ServiceInstance::RateLimit(dynamic_service)
                        | ServiceInstance::RateLimitCheck(dynamic_service)
                        | ServiceInstance::RateLimitReport(dynamic_service) => {
                            let body_values = action.collect_body_values(request_data);
                            if !body_values.is_empty() {
                                tasks.push(Box::new(
                                    TokenUsageTask::with_expected_response_fields(body_values),
                                ));
                            }

                            let task: Box<dyn Task> = Box::new(DynamicTask::new_with_attributes(
                                ctx,
                                action.id.clone(),
                                Rc::clone(dynamic_service),
                                var.clone(),
                                message_builder.clone(),
                                on_reply.clone(),
                                vec![action.predicate.clone()],
                                action.dependencies.clone(),
                                action.is_guard,
                            ));
                            let task = Box::new(FailureModeTask::new(task, abort_on_failure));
                            if tracing_enabled {
                                tasks.push(Box::new(TracingDecoratorTask::new(
                                    label.clone(),
                                    task,
                                    action.sources.clone(),
                                )));
                            } else {
                                tasks.push(task);
                            }
                        }
                    }
                }
                Operation::Deny { deny_with } => {
                    use crate::kuadrant::pipeline::tasks::SendReplyTask;
                    let task = SendReplyTask::new_deferred(
                        action.predicate.clone(),
                        deny_with.clone(),
                        action.terminal,
                    );
                    tasks.push(Box::new(task));
                }
                Operation::Headers {
                    target,
                    headers: headers_expr,
                } => {
                    let task = ModifyHeadersTask::new_deferred(
                        action.predicate.clone(),
                        headers_expr.clone(),
                        target.clone(),
                        action.terminal,
                    );
                    tasks.push(Box::new(task));
                }
                Operation::Store {
                    path,
                    expression,
                    export_to_host,
                } => {
                    use crate::kuadrant::pipeline::tasks::StoreTask;
                    match StoreTask::new(
                        action.predicate.clone(),
                        expression.clone(),
                        path.clone(),
                        *export_to_host,
                        action.terminal,
                    ) {
                        Ok(task) => tasks.push(Box::new(task)),
                        Err(e) => {
                            tracing::error!(
                                "Failed to create StoreTask for path '{}': {}. Action {} will be skipped.",
                                path,
                                e,
                                action.id
                            );
                        }
                    }
                }
                Operation::Fail { log_message } => {
                    tracing::error!(
                        "Top-level Fail operation is currently unsupported. Action {}: {}",
                        action.id,
                        log_message
                    );
                }
            }
        }

        (tasks, teardown_tasks)
    }
}

impl Action {
    #[allow(deprecated)]
    fn compile(
        config: &configuration::Action,
        services: &HashMap<String, ServiceInstance>,
        id: String,
        dependencies: Vec<String>,
        request_data: &[((String, String), String)],
    ) -> Result<Self, CompileError> {
        let service = services
            .get(&config.service)
            .ok_or_else(|| CompileError::UnknownService(config.service.clone()))?;

        let typed_config = match service {
            ServiceInstance::Auth(_) => translate_legacy_auth_to_typed(config, request_data),
            ServiceInstance::RateLimit(_) | ServiceInstance::RateLimitCheck(_) => {
                translate_legacy_ratelimit_to_typed(config, request_data)
            }
            ServiceInstance::RateLimitReport(_) => {
                translate_legacy_report_to_typed(config, request_data)
            }
            _ => {
                return Err(CompileError::ServiceCreationFailed(format!(
                    "Legacy config not supported for service type: {}",
                    config.service
                )))
            }
        };

        Self::compile_typed(&typed_config, services, id, dependencies)
    }

    fn compile_typed(
        typed: &configuration::TypedAction,
        services: &HashMap<String, ServiceInstance>,
        id: String,
        dependencies: Vec<String>,
    ) -> Result<Self, CompileError> {
        let predicate =
            Predicate::new(&typed.predicate).map_err(|e| CompileError::InvalidActionPredicate {
                service: match &typed.operation {
                    configuration::Operation::Grpc(grpc) => grpc.service.clone(),
                    configuration::Operation::Deny(_) => "deny".to_string(),
                    configuration::Operation::Headers(_) => "headers".to_string(),
                    configuration::Operation::Store(_) => "store".to_string(),
                    configuration::Operation::Fail(_) => "fail".to_string(),
                },
                error: e.to_string(),
            })?;

        let operation = match &typed.operation {
            configuration::Operation::Grpc(grpc) => {
                let service_instance = services
                    .get(&grpc.service)
                    .ok_or_else(|| CompileError::UnknownService(grpc.service.clone()))?;

                if !matches!(
                    service_instance,
                    ServiceInstance::Dynamic(_)
                        | ServiceInstance::Auth(_)
                        | ServiceInstance::RateLimit(_)
                        | ServiceInstance::RateLimitCheck(_)
                        | ServiceInstance::RateLimitReport(_)
                ) {
                    return Err(CompileError::ServiceCreationFailed(format!(
                        "Service '{}' cannot be used with gRPC action",
                        grpc.service
                    )));
                }

                let on_reply: Vec<Action> = grpc
                    .on_reply
                    .iter()
                    .enumerate()
                    .map(|(idx, typed_action)| {
                        let reply_id = format!("{}.{}", id, idx);
                        let reply_deps = if idx > 0 {
                            vec![format!("{}.{}", id, idx - 1)]
                        } else {
                            vec![]
                        };
                        Action::compile_typed(typed_action, services, reply_id, reply_deps)
                    })
                    .collect::<Result<_, _>>()?;

                let message_builder = Expression::new(&grpc.message_builder).map_err(|e| {
                    CompileError::InvalidDataExpression(format!(
                        "Failed to compile message_builder: {e}"
                    ))
                })?;

                Operation::Grpc {
                    service: service_instance.clone(),
                    var: grpc.var.clone(),
                    message_builder,
                    on_reply,
                    label: grpc.label.clone(),
                }
            }
            configuration::Operation::Deny(deny) => {
                let deny_with = Expression::new(&deny.deny_with)?;
                Operation::Deny { deny_with }
            }
            configuration::Operation::Headers(headers) => {
                let target = match headers.target {
                    configuration::HeadersTarget::Request => HeadersType::HttpRequestHeaders,
                    configuration::HeadersTarget::Response => HeadersType::HttpResponseHeaders,
                };
                let headers_expr = Expression::new(&headers.headers)?;
                Operation::Headers {
                    target,
                    headers: headers_expr,
                }
            }
            configuration::Operation::Store(store) => {
                let expression = Expression::new(&store.value)?;
                Operation::Store {
                    path: store.path.clone(),
                    expression,
                    export_to_host: store.export_to_host,
                }
            }
            configuration::Operation::Fail(fail) => Operation::Fail {
                log_message: fail.log_message.clone(),
            },
        };

        Ok(Action {
            id,
            predicate,
            terminal: typed.terminal,
            operation,
            dependencies,
            sources: typed.sources.clone(),
            is_guard: typed.is_guard,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configuration::{
        Action as ConfigAction, ActionConfig, ActionSet, ConditionalData as ConfigConditionalData,
        DataItem as ConfigDataItem, DataType, DenyOperation, ExpressionItem, GrpcOperation,
        HeadersOperation, HeadersTarget, Operation as ConfigOperation, RouteRuleConditions,
        StaticItem, StoreOperation, TypedAction as ConfigTypedAction,
    };
    use crate::configuration::{FailOperation, FailureMode};
    use crate::filter::DescriptorManager;
    use crate::services::{DynamicService, ServiceInstance};
    use std::collections::HashMap;
    use std::rc::Rc;

    fn build_test_service(name: &str) -> (String, ServiceInstance) {
        let descriptor_manager = Rc::new(DescriptorManager::default());
        (
            name.to_string(),
            ServiceInstance::Auth(Rc::new(DynamicService::new(
                "test-cluster".to_string(),
                "envoy.service.auth.v3.Authorization".to_string(),
                "Check".to_string(),
                std::time::Duration::from_secs(10),
                FailureMode::Deny,
                descriptor_manager,
            ))),
        )
    }

    fn build_dynamic_service(name: &str) -> (String, ServiceInstance) {
        let descriptor_manager = Rc::new(DescriptorManager::default());
        (
            name.to_string(),
            ServiceInstance::Dynamic(Rc::new(DynamicService::new(
                "test-cluster".to_string(),
                "test.Service".to_string(),
                "TestMethod".to_string(),
                std::time::Duration::from_secs(10),
                FailureMode::Deny,
                descriptor_manager,
            ))),
        )
    }

    #[test]
    fn blueprint_compiles_with_empty_predicates() {
        let services = HashMap::from([build_test_service("test-service")]);

        let config = ActionSet {
            name: "test-action-set".to_string(),
            route_rule_conditions: RouteRuleConditions {
                hostnames: vec!["example.com".to_string()],
                predicates: vec![],
            },
            actions: vec![],
        };

        let result = Blueprint::compile(&config, &services, &[]);
        assert!(result.is_ok());
        let blueprint = result.unwrap();
        assert_eq!(blueprint.name, "test-action-set");
        assert!(blueprint.route_predicates.is_empty());
        assert!(blueprint.actions.is_empty());
    }

    #[test]
    fn blueprint_compiles_valid_route_predicates() {
        let services = HashMap::from([build_test_service("test-service")]);

        let config = ActionSet {
            name: "test-action-set".to_string(),
            route_rule_conditions: RouteRuleConditions {
                hostnames: vec!["example.com".to_string()],
                predicates: vec!["true".to_string(), "request.method == 'GET'".to_string()],
            },
            actions: vec![],
        };

        let result = Blueprint::compile(&config, &services, &[]);
        assert!(result.is_ok());
        let blueprint = result.unwrap();
        assert_eq!(blueprint.route_predicates.len(), 2);
    }

    #[test]
    fn blueprint_fails_on_invalid_route_predicate() {
        let services = HashMap::from([build_test_service("test-service")]);

        let config = ActionSet {
            name: "test-action-set".to_string(),
            route_rule_conditions: RouteRuleConditions {
                hostnames: vec!["example.com".to_string()],
                predicates: vec!["invalid syntax !!@@".to_string()],
            },
            actions: vec![],
        };

        let result = Blueprint::compile(&config, &services, &[]);
        assert!(matches!(
            result,
            Err(CompileError::InvalidRoutePredicate { ref action_set, .. }) if action_set == "test-action-set"
        ));
    }

    #[test]
    fn action_compiles_with_valid_predicates() {
        let services = HashMap::from([build_test_service("test-service")]);

        let config = ConfigAction {
            service: "test-service".to_string(),
            scope: "test-scope".to_string(),
            predicates: vec![
                "true".to_string(),
                "request.path.startsWith('/api')".to_string(),
            ],
            conditional_data: vec![],
            sources: vec![],
        };

        let result = Action::compile(&config, &services, "0".to_string(), vec![], &[]);
        assert!(result.is_ok());
        let action = result.unwrap();
        assert_eq!(action.id, "0");
        assert!(action.dependencies.is_empty());
        assert!(matches!(action.operation, Operation::Grpc { .. }));
    }

    #[test]
    fn action_fails_on_invalid_predicate() {
        let services = HashMap::from([build_test_service("test-service")]);

        let config = ConfigAction {
            service: "test-service".to_string(),
            scope: "test-scope".to_string(),
            predicates: vec!["bad syntax ***".to_string()],
            conditional_data: vec![],
            sources: vec![],
        };

        let result = Action::compile(&config, &services, "0".to_string(), vec![], &[]);
        assert!(matches!(
            result,
            Err(CompileError::InvalidActionPredicate { ref service, .. }) if service == "test-service"
        ));
    }

    #[test]
    fn action_fails_on_unknown_service() {
        let services = HashMap::new();

        let config = ConfigAction {
            service: "nonexistent-service".to_string(),
            scope: "test-scope".to_string(),
            predicates: vec![],
            conditional_data: vec![],
            sources: vec![],
        };

        let result = Action::compile(&config, &services, "0".to_string(), vec![], &[]);
        assert!(matches!(
            result,
            Err(CompileError::UnknownService(ref service)) if service == "nonexistent-service"
        ));
    }

    #[test]
    fn blueprint_compiles_complete_configuration() {
        let services = HashMap::from([build_test_service("auth-service")]);

        let config = ActionSet {
            name: "complete-test".to_string(),
            route_rule_conditions: RouteRuleConditions {
                hostnames: vec!["*.example.com".to_string()],
                predicates: vec!["request.path.startsWith('/api')".to_string()],
            },
            actions: vec![ActionConfig::Legacy(ConfigAction {
                service: "auth-service".to_string(),
                scope: "api-scope".to_string(),
                predicates: vec!["request.method == 'POST'".to_string()],
                conditional_data: vec![ConfigConditionalData {
                    predicates: vec!["request.headers['x-api-key'].size() > 0".to_string()],
                    data: vec![
                        ConfigDataItem {
                            item: DataType::Static(StaticItem {
                                key: "tier".to_string(),
                                value: "premium".to_string(),
                            }),
                        },
                        ConfigDataItem {
                            item: DataType::Expression(ExpressionItem {
                                key: "user".to_string(),
                                value: "auth.identity.username".to_string(),
                            }),
                        },
                    ],
                }],
                sources: vec![],
            })],
        };

        let result = Blueprint::compile(&config, &services, &[]);
        assert!(result.is_ok());
        let blueprint = result.unwrap();
        assert_eq!(blueprint.name, "complete-test");
        assert_eq!(blueprint.route_predicates.len(), 1);
        assert_eq!(blueprint.actions.len(), 1);
        assert!(matches!(
            blueprint.actions[0].operation,
            Operation::Grpc { .. }
        ));
    }

    #[test]
    fn grpc_typed_action_compiles() {
        let services = HashMap::from([build_dynamic_service("my-dynamic")]);

        let typed = ConfigTypedAction {
            predicate: "request.method == 'GET'".to_string(),
            terminal: false,
            is_guard: true,
            sources: vec![],
            operation: ConfigOperation::Grpc(GrpcOperation {
                var: "rl_check".to_string(),
                service: "my-dynamic".to_string(),
                message_builder: "envoy.service.ratelimit.v3.RateLimitRequest{}".to_string(),
                on_reply: vec![
                    ConfigTypedAction {
                        predicate: "rl_check.overall_code == 2".to_string(),
                        terminal: true,
                        is_guard: false,
                        sources: vec![],
                        operation: ConfigOperation::Deny(DenyOperation {
                            deny_with: "DenyResponse{status: 429u}".to_string(),
                        }),
                    },
                    ConfigTypedAction {
                        predicate: "rl_check.overall_code == 0".to_string(),
                        terminal: true,
                        is_guard: false,
                        sources: vec![],
                        operation: ConfigOperation::Fail(FailOperation {
                            log_message: "Received UNKNOWN from rate limiting service".to_string(),
                        }),
                    },
                    ConfigTypedAction {
                        predicate: "rl_check.overall_code != 1 && rl_check.overall_code != 2"
                            .to_string(),
                        terminal: true,
                        is_guard: false,
                        sources: vec![],
                        operation: ConfigOperation::Fail(FailOperation {
                            log_message:
                                "Received invalid response code from rate limiting service"
                                    .to_string(),
                        }),
                    },
                    ConfigTypedAction {
                        predicate: "true".to_string(),
                        terminal: false,
                        is_guard: false,
                        sources: vec![],
                        operation: ConfigOperation::Headers(HeadersOperation {
                            target: HeadersTarget::Request,
                            headers: "result.headers".to_string(),
                        }),
                    },
                    ConfigTypedAction {
                        predicate: "true".to_string(),
                        terminal: false,
                        is_guard: false,
                        sources: vec![],
                        operation: ConfigOperation::Store(StoreOperation {
                            path: "rl.remaining".to_string(),
                            value: "result.remaining".to_string(),
                            export_to_host: false,
                        }),
                    },
                ],
                label: String::new(),
            }),
        };

        let result = Action::compile_typed(&typed, &services, "0".to_string(), vec![]);
        assert!(result.is_ok());
        let action = result.unwrap();
        assert_eq!(action.id, "0");
        assert!(action.is_guard);
        assert!(!action.terminal);
        assert!(matches!(action.operation, Operation::Grpc { .. }));
        if let Operation::Grpc {
            ref service,
            ref var,
            ref on_reply,
            ..
        } = action.operation
        {
            assert_eq!(var, "rl_check");
            assert!(matches!(service, ServiceInstance::Dynamic(_)));
            assert_eq!(on_reply.len(), 5);
        }
    }

    #[test]
    fn grpc_typed_action_fails_on_unknown_service() {
        let services = HashMap::new();

        let typed = ConfigTypedAction {
            predicate: "true".to_string(),
            terminal: false,
            is_guard: true,
            sources: vec![],
            operation: ConfigOperation::Grpc(GrpcOperation {
                var: "check".to_string(),
                service: "nonexistent".to_string(),
                message_builder: "test.Request{}".to_string(),
                on_reply: vec![],
                label: String::new(),
            }),
        };

        let result = Action::compile_typed(&typed, &services, "0".to_string(), vec![]);
        assert!(matches!(result, Err(CompileError::UnknownService(ref s)) if s == "nonexistent"));
    }

    #[test]
    fn grpc_typed_action_fails_on_non_dynamic_service() {
        use crate::services::TracingService;
        let services = HashMap::from([(
            "tracing-svc".to_string(),
            ServiceInstance::Tracing(Some(Rc::new(TracingService::new(
                "test-cluster".to_string(),
                std::time::Duration::from_secs(10),
            )))),
        )]);

        let typed = ConfigTypedAction {
            predicate: "true".to_string(),
            terminal: false,
            is_guard: true,
            sources: vec![],
            operation: ConfigOperation::Grpc(GrpcOperation {
                var: "check".to_string(),
                service: "tracing-svc".to_string(),
                message_builder: "test.Request{}".to_string(),
                on_reply: vec![],
                label: String::new(),
            }),
        };

        let result = Action::compile_typed(&typed, &services, "0".to_string(), vec![]);
        assert!(matches!(
            result,
            Err(CompileError::ServiceCreationFailed(_))
        ));
    }

    #[test]
    fn grpc_in_on_reply_block_compiles() {
        let services = HashMap::from([build_dynamic_service("svc")]);

        let nested_grpc = ConfigTypedAction {
            predicate: "true".to_string(),
            terminal: false,
            is_guard: false,
            sources: vec![],
            operation: ConfigOperation::Grpc(GrpcOperation {
                var: "nested".to_string(),
                service: "svc".to_string(),
                message_builder: "test.Request{}".to_string(),
                on_reply: vec![],
                label: String::new(),
            }),
        };

        let result = Action::compile_typed(&nested_grpc, &services, "parent.0".to_string(), vec![]);
        assert!(result.is_ok());
        let action = result.unwrap();
        assert_eq!(action.id, "parent.0");
        assert!(matches!(action.operation, Operation::Grpc { .. }));
    }

    #[test]
    fn typed_actions_compile() {
        let services = HashMap::new();

        let deny_config = ConfigTypedAction {
            predicate: "result.code == 2".to_string(),
            terminal: true,
            is_guard: false,
            sources: vec![],
            operation: ConfigOperation::Deny(DenyOperation {
                deny_with: "DenyResponse{status: 429u}".to_string(),
            }),
        };
        let deny_result = Action::compile_typed(&deny_config, &services, "0".to_string(), vec![]);
        assert!(deny_result.is_ok());
        let deny = deny_result.unwrap();
        assert!(deny.terminal);
        assert!(matches!(deny.operation, Operation::Deny { .. }));

        let headers_config = ConfigTypedAction {
            predicate: "true".to_string(),
            terminal: false,
            is_guard: false,
            sources: vec![],
            operation: ConfigOperation::Headers(HeadersOperation {
                target: HeadersTarget::Response,
                headers: "result.resp_headers".to_string(),
            }),
        };
        let headers_result =
            Action::compile_typed(&headers_config, &services, "0".to_string(), vec![]);
        assert!(headers_result.is_ok());
        let headers = headers_result.unwrap();
        assert!(!headers.terminal);
        assert!(matches!(
            headers.operation,
            Operation::Headers {
                ref target,
                ..
            } if matches!(target, HeadersType::HttpResponseHeaders)
        ));

        let store_config = ConfigTypedAction {
            predicate: "true".to_string(),
            terminal: false,
            is_guard: true,
            sources: vec![],
            operation: ConfigOperation::Store(StoreOperation {
                path: "a.b".to_string(),
                value: "result.x".to_string(),
                export_to_host: false,
            }),
        };
        let store_result = Action::compile_typed(&store_config, &services, "0".to_string(), vec![]);
        assert!(store_result.is_ok());
        let store = store_result.unwrap();
        assert!(!store.terminal);
        assert!(matches!(
            store.operation,
            Operation::Store { ref path, .. } if path == "a.b"
        ));
    }

    #[test]
    fn typed_action_fails_on_invalid_predicate() {
        let services = HashMap::new();

        let config = ConfigTypedAction {
            predicate: "bad syntax !!".to_string(),
            terminal: true,
            is_guard: true,
            sources: vec![],
            operation: ConfigOperation::Deny(DenyOperation {
                deny_with: "DenyResponse{status: 429u}".to_string(),
            }),
        };
        let result = Action::compile_typed(&config, &services, "0".to_string(), vec![]);
        assert!(matches!(
            result,
            Err(CompileError::InvalidActionPredicate { .. })
        ));
    }

    #[test]
    fn mixed_legacy_and_typed_actions_compile() {
        let services = HashMap::from([
            build_test_service("auth-svc"),
            build_dynamic_service("dyn-svc"),
        ]);

        let config = ActionSet {
            name: "mixed-set".to_string(),
            route_rule_conditions: RouteRuleConditions {
                hostnames: vec!["example.com".to_string()],
                predicates: vec![],
            },
            actions: vec![
                ActionConfig::Legacy(ConfigAction {
                    service: "auth-svc".to_string(),
                    scope: "auth-scope".to_string(),
                    predicates: vec![],
                    conditional_data: vec![],
                    sources: vec![],
                }),
                ActionConfig::Typed(ConfigTypedAction {
                    predicate: "true".to_string(),
                    terminal: false,
                    is_guard: true,
                    sources: vec![],
                    operation: ConfigOperation::Grpc(GrpcOperation {
                        var: "rl_check".to_string(),
                        service: "dyn-svc".to_string(),
                        message_builder: "test.Request{}".to_string(),
                        on_reply: vec![ConfigTypedAction {
                            predicate: "rl_check.code == 2".to_string(),
                            terminal: true,
                            is_guard: false,
                            sources: vec![],
                            operation: ConfigOperation::Deny(DenyOperation {
                                deny_with: "DenyResponse{status: 429u}".to_string(),
                            }),
                        }],
                        label: String::new(),
                    }),
                }),
            ],
        };

        let result = Blueprint::compile(&config, &services, &[]);
        assert!(result.is_ok());
        let blueprint = result.unwrap();
        assert_eq!(blueprint.actions.len(), 2);

        assert!(matches!(
            &blueprint.actions[0].operation,
            Operation::Grpc { .. }
        ));
        if let Operation::Grpc { service, .. } = &blueprint.actions[0].operation {
            assert!(matches!(service, ServiceInstance::Auth(_)));
        }
        assert!(blueprint.actions[0].dependencies.is_empty());

        assert!(matches!(
            &blueprint.actions[1].operation,
            Operation::Grpc { .. }
        ));
        if let Operation::Grpc {
            service, on_reply, ..
        } = &blueprint.actions[1].operation
        {
            assert!(matches!(service, ServiceInstance::Dynamic(_)));
            assert_eq!(on_reply.len(), 1);
        }
        assert_eq!(blueprint.actions[1].dependencies, vec!["0"]);
    }
}
