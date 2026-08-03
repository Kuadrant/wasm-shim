use crate::configuration;
use crate::data::{cel::Predicate, Expression};
use crate::kuadrant::pipeline::tasks::{
    DynamicTask, ExportTracesTask, FailureModeTask, HeadersType, ModifyHeadersTask, Task,
    TeardownAction, TokenUsageTask, TracingDecoratorTask,
};
use crate::kuadrant::ReqRespCtx;
use crate::services::ServiceInstance;
use cel::ParseErrors;
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::Arc;

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
    fn to_core_task(&self, ctx: &mut ReqRespCtx) -> Option<Box<dyn Task>> {
        match &self.operation {
            Operation::Grpc {
                service,
                var,
                message_builder,
                on_reply,
                label: _,
            } => {
                let children: Vec<Box<dyn Task>> = on_reply
                    .iter()
                    .filter_map(|a| a.to_core_task(ctx))
                    .collect();

                match service {
                    ServiceInstance::Dynamic(dynamic_service)
                    | ServiceInstance::Auth(dynamic_service)
                    | ServiceInstance::RateLimit(dynamic_service)
                    | ServiceInstance::RateLimitCheck(dynamic_service)
                    | ServiceInstance::RateLimitReport(dynamic_service) => {
                        Some(Box::new(DynamicTask::new_with_attributes(
                            ctx,
                            self.id.clone(),
                            Arc::clone(dynamic_service),
                            var.clone(),
                            message_builder.clone(),
                            children,
                            self.predicate.clone(),
                            self.dependencies.clone(),
                            self.is_guard,
                        )))
                    }
                    ServiceInstance::Tracing(_) => {
                        ctx.set_public_tracker_id(var.clone());
                        #[allow(clippy::expect_used)]
                        let predicate = Predicate::new("true").expect("Needs to be valid!");
                        #[allow(clippy::expect_used)]
                        let headers_expr =
                            Expression::new(&format!("[['{var}', '{}']]", ctx.request_id()))
                                .expect("Needs to be valid CEL!");
                        Some(Box::new(ModifyHeadersTask::new(
                            self.id.clone(),
                            predicate,
                            headers_expr,
                            HeadersType::HttpResponseHeaders,
                            false,
                        )))
                    }
                }
            }
            Operation::Deny { deny_with } => {
                use crate::kuadrant::pipeline::tasks::SendReplyTask;
                Some(Box::new(SendReplyTask::new(
                    self.id.clone(),
                    self.predicate.clone(),
                    deny_with.clone(),
                    self.terminal,
                )))
            }
            Operation::Headers {
                target,
                headers: headers_expr,
            } => Some(Box::new(ModifyHeadersTask::new(
                self.id.clone(),
                self.predicate.clone(),
                headers_expr.clone(),
                target.clone(),
                self.terminal,
            ))),
            Operation::Store {
                path,
                expression,
                export_to_host,
            } => {
                use crate::kuadrant::pipeline::tasks::StoreTask;
                match StoreTask::new(
                    ctx,
                    self.id.clone(),
                    self.predicate.clone(),
                    expression.clone(),
                    path.clone(),
                    *export_to_host,
                    self.terminal,
                ) {
                    Ok(task) => Some(Box::new(task)),
                    Err(e) => {
                        tracing::error!(
                            "Failed to create StoreTask for path '{}': {}. Action {} will be skipped.",
                            path,
                            e,
                            self.id
                        );
                        None
                    }
                }
            }
            Operation::Fail { log_message } => {
                use crate::kuadrant::pipeline::tasks::FailTask;
                Some(Box::new(FailTask::new(
                    self.id.clone(),
                    self.predicate.clone(),
                    log_message.clone(),
                    self.terminal,
                )))
            }
        }
    }

    pub fn collect_body_values(&self) -> Vec<String> {
        use std::collections::HashSet;

        let mut fields = HashSet::new();

        fields.extend(self.predicate.response_body_values().iter().cloned());

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
                                    .flat_map(|nested| nested.collect_body_values()),
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
    InvalidOnReplyExecution(String),
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
            CompileError::InvalidOnReplyExecution(id) => {
                write!(f, "on_reply action '{}' must not set execution mode", id)
            }
        }
    }
}

impl Blueprint {
    pub fn compile(
        config: &configuration::ActionSet,
        services: &HashMap<String, ServiceInstance>,
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
            .zip(compute_dependencies(&config.actions))
            .enumerate()
            .map(
                |(i, (action_config, dependencies))| -> Result<Action, CompileError> {
                    let id = i.to_string();
                    let mut action = Action::compile(action_config, services, id)?;
                    action.dependencies = dependencies;
                    Ok(action)
                },
            )
            .collect::<Result<_, _>>()?;

        Ok(Self {
            name: config.name.clone(),
            route_predicates,
            actions,
        })
    }
}

fn compute_dependencies(actions: &[configuration::Action]) -> Vec<Vec<String>> {
    let mut last_fence: Vec<String> = vec![];
    let mut pending_parallel: Vec<String> = vec![];
    let mut result = Vec::with_capacity(actions.len());

    for (i, action) in actions.iter().enumerate() {
        let id = i.to_string();
        if action.execution == configuration::Execution::Sequential {
            let deps: Vec<String> = last_fence
                .iter()
                .chain(&pending_parallel)
                .cloned()
                .collect();
            last_fence = vec![id];
            pending_parallel = vec![];
            result.push(deps);
        } else {
            let deps = last_fence.clone();
            pending_parallel.push(id);
            result.push(deps);
        }
    }

    result
}

type TaskList = Vec<Box<dyn Task>>;
type TeardownList = Vec<Box<dyn TeardownAction>>;

impl Blueprint {
    pub fn to_tasks(&self, ctx: &mut ReqRespCtx) -> (TaskList, TeardownList) {
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
                Operation::Grpc { service, label, .. } => match service {
                    ServiceInstance::Tracing(tracing_service) => {
                        if let Some(task) = action.to_core_task(ctx) {
                            tasks.push(task);
                        }
                        if let Some(service) = tracing_service {
                            teardown_tasks
                                .push(Box::new(ExportTracesTask::new(ctx, Arc::clone(service))));
                        }
                    }
                    ServiceInstance::Dynamic(_)
                    | ServiceInstance::Auth(_)
                    | ServiceInstance::RateLimit(_)
                    | ServiceInstance::RateLimitCheck(_)
                    | ServiceInstance::RateLimitReport(_) => {
                        let body_values = action.collect_body_values();
                        if !body_values.is_empty() {
                            tasks.push(Box::new(TokenUsageTask::with_expected_response_fields(
                                body_values,
                            )));
                        }

                        if let Some(mut task) = action.to_core_task(ctx) {
                            let abort_on_failure =
                                service.failure_mode() == configuration::FailureMode::Deny;
                            task = Box::new(FailureModeTask::new(task, abort_on_failure));

                            if tracing_enabled {
                                task = Box::new(TracingDecoratorTask::new(
                                    label.clone(),
                                    task,
                                    action.sources.clone(),
                                ));
                            }

                            tasks.push(task);
                        }
                    }
                },
                _ => {
                    if let Some(task) = action.to_core_task(ctx) {
                        tasks.push(task);
                    }
                }
            }
        }

        (tasks, teardown_tasks)
    }
}

impl Action {
    fn compile(
        typed: &configuration::Action,
        services: &HashMap<String, ServiceInstance>,
        id: String,
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
                    .map(|(idx, reply_action)| {
                        let reply_id = format!("{}.{}", id, idx);
                        if reply_action.execution != configuration::Execution::default() {
                            return Err(CompileError::InvalidOnReplyExecution(reply_id));
                        }
                        Action::compile(reply_action, services, reply_id)
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
            dependencies: vec![],
            sources: typed.sources.clone(),
            is_guard: typed.is_guard,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configuration::{
        Action as ConfigAction, ActionSet, DenyOperation, Execution, FailOperation, FailureMode,
        GrpcOperation, HeadersOperation, HeadersTarget, Operation as ConfigOperation,
        RouteRuleConditions, StoreOperation,
    };
    use crate::filter::DescriptorManager;
    use crate::services::{DynamicService, ServiceInstance};
    use std::collections::HashMap;
    use std::sync::Arc;

    fn build_test_service(name: &str) -> (String, ServiceInstance) {
        let descriptor_manager = Arc::new(DescriptorManager::default());
        (
            name.to_string(),
            ServiceInstance::Dynamic(Arc::new(DynamicService::new(
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
        let descriptor_manager = Arc::new(DescriptorManager::default());
        (
            name.to_string(),
            ServiceInstance::Dynamic(Arc::new(DynamicService::new(
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

        let result = Blueprint::compile(&config, &services);
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

        let result = Blueprint::compile(&config, &services);
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

        let result = Blueprint::compile(&config, &services);
        assert!(matches!(
            result,
            Err(CompileError::InvalidRoutePredicate { ref action_set, .. }) if action_set == "test-action-set"
        ));
    }

    #[test]
    fn grpc_action_compiles() {
        let services = HashMap::from([build_dynamic_service("my-dynamic")]);

        let config = ConfigAction {
            predicate: "request.method == 'GET'".to_string(),
            terminal: false,
            is_guard: true,
            sources: vec![],
            execution: Execution::default(),
            operation: ConfigOperation::Grpc(GrpcOperation {
                var: "rl_check".to_string(),
                service: "my-dynamic".to_string(),
                message_builder: "envoy.service.ratelimit.v3.RateLimitRequest{}".to_string(),
                on_reply: vec![
                    ConfigAction {
                        predicate: "rl_check.overall_code == 2".to_string(),
                        terminal: true,
                        is_guard: false,
                        sources: vec![],
                        execution: Execution::default(),
                        operation: ConfigOperation::Deny(DenyOperation {
                            deny_with: "DenyResponse{status: 429u}".to_string(),
                        }),
                    },
                    ConfigAction {
                        predicate: "rl_check.overall_code == 0".to_string(),
                        terminal: true,
                        is_guard: false,
                        sources: vec![],
                        execution: Execution::default(),
                        operation: ConfigOperation::Fail(FailOperation {
                            log_message: "Received UNKNOWN from rate limiting service".to_string(),
                        }),
                    },
                    ConfigAction {
                        predicate: "rl_check.overall_code != 1 && rl_check.overall_code != 2"
                            .to_string(),
                        terminal: true,
                        is_guard: false,
                        sources: vec![],
                        execution: Execution::default(),
                        operation: ConfigOperation::Fail(FailOperation {
                            log_message:
                                "Received invalid response code from rate limiting service"
                                    .to_string(),
                        }),
                    },
                    ConfigAction {
                        predicate: "true".to_string(),
                        terminal: false,
                        is_guard: false,
                        sources: vec![],
                        execution: Execution::default(),
                        operation: ConfigOperation::Headers(HeadersOperation {
                            target: HeadersTarget::Request,
                            headers: "result.headers".to_string(),
                        }),
                    },
                    ConfigAction {
                        predicate: "true".to_string(),
                        terminal: false,
                        is_guard: false,
                        sources: vec![],
                        execution: Execution::default(),
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

        let result = Action::compile(&config, &services, "0".to_string());
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
    fn grpc_action_fails_on_unknown_service() {
        let services = HashMap::new();

        let config = ConfigAction {
            predicate: "true".to_string(),
            terminal: false,
            is_guard: true,
            sources: vec![],
            execution: Execution::default(),
            operation: ConfigOperation::Grpc(GrpcOperation {
                var: "check".to_string(),
                service: "nonexistent".to_string(),
                message_builder: "test.Request{}".to_string(),
                on_reply: vec![],
                label: String::new(),
            }),
        };

        let result = Action::compile(&config, &services, "0".to_string());
        assert!(matches!(result, Err(CompileError::UnknownService(ref s)) if s == "nonexistent"));
    }

    #[test]
    fn grpc_action_fails_on_non_dynamic_service() {
        use crate::services::TracingService;
        let services = HashMap::from([(
            "tracing-svc".to_string(),
            ServiceInstance::Tracing(Some(Arc::new(TracingService::new(
                "test-cluster".to_string(),
                std::time::Duration::from_secs(10),
            )))),
        )]);

        let config = ConfigAction {
            predicate: "true".to_string(),
            terminal: false,
            is_guard: true,
            sources: vec![],
            execution: Execution::default(),
            operation: ConfigOperation::Grpc(GrpcOperation {
                var: "check".to_string(),
                service: "tracing-svc".to_string(),
                message_builder: "test.Request{}".to_string(),
                on_reply: vec![],
                label: String::new(),
            }),
        };

        let result = Action::compile(&config, &services, "0".to_string());
        assert!(matches!(
            result,
            Err(CompileError::ServiceCreationFailed(_))
        ));
    }

    #[test]
    fn grpc_in_on_reply_block_compiles() {
        let services = HashMap::from([build_dynamic_service("svc")]);

        let nested_grpc = ConfigAction {
            predicate: "true".to_string(),
            terminal: false,
            is_guard: false,
            sources: vec![],
            execution: Execution::default(),
            operation: ConfigOperation::Grpc(GrpcOperation {
                var: "nested".to_string(),
                service: "svc".to_string(),
                message_builder: "test.Request{}".to_string(),
                on_reply: vec![],
                label: String::new(),
            }),
        };

        let result = Action::compile(&nested_grpc, &services, "parent.0".to_string());
        assert!(result.is_ok());
        let action = result.unwrap();
        assert_eq!(action.id, "parent.0");
        assert!(matches!(action.operation, Operation::Grpc { .. }));
    }

    #[test]
    fn actions_compile() {
        let services = HashMap::new();

        let deny_config = ConfigAction {
            predicate: "result.code == 2".to_string(),
            terminal: true,
            is_guard: false,
            sources: vec![],
            execution: Execution::default(),
            operation: ConfigOperation::Deny(DenyOperation {
                deny_with: "DenyResponse{status: 429u}".to_string(),
            }),
        };
        let deny_result = Action::compile(&deny_config, &services, "0".to_string());
        assert!(deny_result.is_ok());
        let deny = deny_result.unwrap();
        assert!(deny.terminal);
        assert!(matches!(deny.operation, Operation::Deny { .. }));

        let headers_config = ConfigAction {
            predicate: "true".to_string(),
            terminal: false,
            is_guard: false,
            sources: vec![],
            execution: Execution::default(),
            operation: ConfigOperation::Headers(HeadersOperation {
                target: HeadersTarget::Response,
                headers: "result.resp_headers".to_string(),
            }),
        };
        let headers_result = Action::compile(&headers_config, &services, "0".to_string());
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

        let store_config = ConfigAction {
            predicate: "true".to_string(),
            terminal: false,
            is_guard: true,
            sources: vec![],
            execution: Execution::default(),
            operation: ConfigOperation::Store(StoreOperation {
                path: "a.b".to_string(),
                value: "result.x".to_string(),
                export_to_host: false,
            }),
        };
        let store_result = Action::compile(&store_config, &services, "0".to_string());
        assert!(store_result.is_ok());
        let store = store_result.unwrap();
        assert!(!store.terminal);
        assert!(matches!(
            store.operation,
            Operation::Store { ref path, .. } if path == "a.b"
        ));
    }

    #[test]
    fn action_fails_on_invalid_predicate() {
        let services = HashMap::new();

        let config = ConfigAction {
            predicate: "bad syntax !!".to_string(),
            terminal: true,
            is_guard: true,
            sources: vec![],
            execution: Execution::default(),
            operation: ConfigOperation::Deny(DenyOperation {
                deny_with: "DenyResponse{status: 429u}".to_string(),
            }),
        };
        let result = Action::compile(&config, &services, "0".to_string());
        assert!(matches!(
            result,
            Err(CompileError::InvalidActionPredicate { .. })
        ));
    }

    #[test]
    fn action_uses_positional_id() {
        let services = HashMap::from([build_dynamic_service("svc")]);

        let config = ActionSet {
            name: "positional-id-test".to_string(),
            route_rule_conditions: RouteRuleConditions {
                hostnames: vec!["example.com".to_string()],
                predicates: vec![],
            },
            actions: vec![ConfigAction {
                predicate: "true".to_string(),
                terminal: false,
                is_guard: true,
                sources: vec![],
                execution: Execution::default(),
                operation: ConfigOperation::Grpc(GrpcOperation {
                    var: "rl".to_string(),
                    service: "svc".to_string(),
                    message_builder: "test.Request{}".to_string(),
                    on_reply: vec![],
                    label: String::new(),
                }),
            }],
        };

        let blueprint = Blueprint::compile(&config, &services).unwrap();
        assert_eq!(blueprint.actions[0].id, "0");
    }

    #[test]
    fn on_reply_uses_positional_id() {
        let services = HashMap::from([build_dynamic_service("svc")]);

        let config = ConfigAction {
            predicate: "true".to_string(),
            terminal: false,
            is_guard: true,
            sources: vec![],
            execution: Execution::default(),
            operation: ConfigOperation::Grpc(GrpcOperation {
                var: "rl".to_string(),
                service: "svc".to_string(),
                message_builder: "test.Request{}".to_string(),
                on_reply: vec![ConfigAction {
                    predicate: "true".to_string(),
                    terminal: false,
                    is_guard: false,
                    sources: vec![],
                    execution: Execution::default(),
                    operation: ConfigOperation::Deny(DenyOperation {
                        deny_with: "DenyResponse{status: 429u}".to_string(),
                    }),
                }],
                label: String::new(),
            }),
        };

        let result = Action::compile(&config, &services, "parent".to_string());
        assert!(result.is_ok());
        let action = result.unwrap();
        if let Operation::Grpc { on_reply, .. } = &action.operation {
            assert_eq!(on_reply[0].id, "parent.0");
        } else {
            unreachable!("expected grpc operation");
        }
    }

    #[test]
    fn sequential_action_depends_on_all_prior() {
        let services = HashMap::from([build_dynamic_service("svc")]);

        let config = ActionSet {
            name: "seq-test".to_string(),
            route_rule_conditions: RouteRuleConditions {
                hostnames: vec!["example.com".to_string()],
                predicates: vec![],
            },
            actions: vec![
                ConfigAction {
                    predicate: "true".to_string(),
                    terminal: false,
                    is_guard: true,
                    sources: vec![],
                    execution: Execution::Sequential,
                    operation: ConfigOperation::Grpc(GrpcOperation {
                        var: "rl".to_string(),
                        service: "svc".to_string(),
                        message_builder: "test.Request{}".to_string(),
                        on_reply: vec![],
                        label: String::new(),
                    }),
                },
                ConfigAction {
                    predicate: "true".to_string(),
                    terminal: false,
                    is_guard: true,
                    sources: vec![],
                    execution: Execution::Parallel,
                    operation: ConfigOperation::Grpc(GrpcOperation {
                        var: "auth".to_string(),
                        service: "svc".to_string(),
                        message_builder: "test.Request{}".to_string(),
                        on_reply: vec![],
                        label: String::new(),
                    }),
                },
            ],
        };

        let blueprint = Blueprint::compile(&config, &services).unwrap();
        assert!(blueprint.actions[0].dependencies.is_empty());
        assert_eq!(blueprint.actions[1].dependencies, vec!["0"]);
    }

    #[test]
    fn parallel_actions_share_fence_dependency() {
        let services = HashMap::from([build_dynamic_service("svc")]);

        let config = ActionSet {
            name: "par-test".to_string(),
            route_rule_conditions: RouteRuleConditions {
                hostnames: vec!["example.com".to_string()],
                predicates: vec![],
            },
            actions: vec![
                ConfigAction {
                    predicate: "true".to_string(),
                    terminal: false,
                    is_guard: true,
                    sources: vec![],
                    execution: Execution::Sequential,
                    operation: ConfigOperation::Grpc(GrpcOperation {
                        var: "rl".to_string(),
                        service: "svc".to_string(),
                        message_builder: "test.Request{}".to_string(),
                        on_reply: vec![],
                        label: String::new(),
                    }),
                },
                ConfigAction {
                    predicate: "true".to_string(),
                    terminal: false,
                    is_guard: true,
                    sources: vec![],
                    execution: Execution::Parallel,
                    operation: ConfigOperation::Grpc(GrpcOperation {
                        var: "auth".to_string(),
                        service: "svc".to_string(),
                        message_builder: "test.Request{}".to_string(),
                        on_reply: vec![],
                        label: String::new(),
                    }),
                },
                ConfigAction {
                    predicate: "true".to_string(),
                    terminal: false,
                    is_guard: true,
                    sources: vec![],
                    execution: Execution::Parallel,
                    operation: ConfigOperation::Grpc(GrpcOperation {
                        var: "custom".to_string(),
                        service: "svc".to_string(),
                        message_builder: "test.Request{}".to_string(),
                        on_reply: vec![],
                        label: String::new(),
                    }),
                },
            ],
        };

        let blueprint = Blueprint::compile(&config, &services).unwrap();
        assert!(blueprint.actions[0].dependencies.is_empty());
        assert_eq!(blueprint.actions[1].dependencies, vec!["0"]);
        assert_eq!(blueprint.actions[2].dependencies, vec!["0"]);
    }

    #[test]
    fn second_sequential_collects_all_pending() {
        let services = HashMap::from([build_dynamic_service("svc")]);

        let config = ActionSet {
            name: "fence-test".to_string(),
            route_rule_conditions: RouteRuleConditions {
                hostnames: vec!["example.com".to_string()],
                predicates: vec![],
            },
            actions: vec![
                ConfigAction {
                    predicate: "true".to_string(),
                    terminal: false,
                    is_guard: true,
                    sources: vec![],
                    execution: Execution::Sequential,
                    operation: ConfigOperation::Grpc(GrpcOperation {
                        var: "a".to_string(),
                        service: "svc".to_string(),
                        message_builder: "test.Request{}".to_string(),
                        on_reply: vec![],
                        label: String::new(),
                    }),
                },
                ConfigAction {
                    predicate: "true".to_string(),
                    terminal: false,
                    is_guard: true,
                    sources: vec![],
                    execution: Execution::Parallel,
                    operation: ConfigOperation::Grpc(GrpcOperation {
                        var: "b".to_string(),
                        service: "svc".to_string(),
                        message_builder: "test.Request{}".to_string(),
                        on_reply: vec![],
                        label: String::new(),
                    }),
                },
                ConfigAction {
                    predicate: "true".to_string(),
                    terminal: false,
                    is_guard: true,
                    sources: vec![],
                    execution: Execution::Parallel,
                    operation: ConfigOperation::Grpc(GrpcOperation {
                        var: "c".to_string(),
                        service: "svc".to_string(),
                        message_builder: "test.Request{}".to_string(),
                        on_reply: vec![],
                        label: String::new(),
                    }),
                },
                ConfigAction {
                    predicate: "true".to_string(),
                    terminal: false,
                    is_guard: true,
                    sources: vec![],
                    execution: Execution::Sequential,
                    operation: ConfigOperation::Grpc(GrpcOperation {
                        var: "d".to_string(),
                        service: "svc".to_string(),
                        message_builder: "test.Request{}".to_string(),
                        on_reply: vec![],
                        label: String::new(),
                    }),
                },
            ],
        };

        let blueprint = Blueprint::compile(&config, &services).unwrap();
        assert!(blueprint.actions[0].dependencies.is_empty());
        assert_eq!(blueprint.actions[1].dependencies, vec!["0"]);
        assert_eq!(blueprint.actions[2].dependencies, vec!["0"]);
        let mut d_deps = blueprint.actions[3].dependencies.clone();
        d_deps.sort();
        assert_eq!(d_deps, vec!["0", "1", "2"]);
    }

    #[test]
    fn all_parallel_actions_have_no_dependencies() {
        let services = HashMap::from([build_dynamic_service("svc")]);

        let config = ActionSet {
            name: "all-par-test".to_string(),
            route_rule_conditions: RouteRuleConditions {
                hostnames: vec!["example.com".to_string()],
                predicates: vec![],
            },
            actions: vec![
                ConfigAction {
                    predicate: "true".to_string(),
                    terminal: false,
                    is_guard: true,
                    sources: vec![],
                    execution: Execution::Parallel,
                    operation: ConfigOperation::Grpc(GrpcOperation {
                        var: "a".to_string(),
                        service: "svc".to_string(),
                        message_builder: "test.Request{}".to_string(),
                        on_reply: vec![],
                        label: String::new(),
                    }),
                },
                ConfigAction {
                    predicate: "true".to_string(),
                    terminal: false,
                    is_guard: true,
                    sources: vec![],
                    execution: Execution::Parallel,
                    operation: ConfigOperation::Grpc(GrpcOperation {
                        var: "b".to_string(),
                        service: "svc".to_string(),
                        message_builder: "test.Request{}".to_string(),
                        on_reply: vec![],
                        label: String::new(),
                    }),
                },
            ],
        };

        let blueprint = Blueprint::compile(&config, &services).unwrap();
        assert!(blueprint.actions[0].dependencies.is_empty());
        assert!(blueprint.actions[1].dependencies.is_empty());
    }

    #[test]
    fn on_reply_rejects_non_default_execution() {
        let services = HashMap::from([build_dynamic_service("svc")]);

        let action = ConfigAction {
            predicate: "true".to_string(),
            terminal: false,
            is_guard: true,
            sources: vec![],
            execution: Execution::default(),
            operation: ConfigOperation::Grpc(GrpcOperation {
                var: "rl".to_string(),
                service: "svc".to_string(),
                message_builder: "test.Request{}".to_string(),
                on_reply: vec![ConfigAction {
                    predicate: "true".to_string(),
                    terminal: false,
                    is_guard: false,
                    sources: vec![],
                    execution: Execution::Sequential,
                    operation: ConfigOperation::Store(StoreOperation {
                        path: "result".to_string(),
                        value: "true".to_string(),
                        export_to_host: false,
                    }),
                }],
                label: String::new(),
            }),
        };

        let result = Action::compile(&action, &services, "0".to_string());
        assert!(matches!(
            result,
            Err(CompileError::InvalidOnReplyExecution(_))
        ));
    }
}
