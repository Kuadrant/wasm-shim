use crate::configuration;
use crate::data::{cel::Predicate, Expression};
use crate::kuadrant::pipeline::tasks::{
    AuthTask, DynamicTask, ExportTracesTask, FailureModeTask, HeaderOperation, HeadersType,
    ModifyHeadersTask, RateLimitTask, Task, TeardownAction, TokenUsageTask, TracingDecoratorTask,
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
    pub service: ServiceInstance,
    pub scope: String,
    pub predicates: Vec<Predicate>,
    pub conditional_data: Vec<ConditionalData>,
    pub dependencies: Vec<String>,
    pub sources: Vec<String>,
    pub message_builder: Option<Expression>,
    pub on_reply: Vec<TypedAction>,
}

// todo(@adam-cattermole): collapse TypedAction into Action once built-in services are migrated to DynamicTask
#[derive(Clone)]
pub(crate) struct TypedAction {
    pub predicate: Predicate,
    pub terminal: bool,
    pub operation: Operation,
}

#[derive(Clone)]
pub(crate) enum Operation {
    Deny {
        deny_with: Expression,
    },
    Headers {
        target: HeadersType,
        headers: Expression,
    },
    Store {
        data: Vec<(String, Expression)>,
    },
}

impl Action {
    pub fn collect_body_values(&self, request_data: &[RequestData]) -> Vec<String> {
        let mut fields = Vec::new();

        for predicate in &self.predicates {
            fields.extend(predicate.body_values().iter().cloned());
        }
        for data in &self.conditional_data {
            for predicate in &data.predicates {
                fields.extend(predicate.body_values().iter().cloned());
            }
            for item in &data.data {
                fields.extend(item.value.body_values().iter().cloned());
            }
        }
        for (_, expr) in request_data {
            fields.extend(expr.body_values().iter().cloned());
        }

        fields.dedup();
        fields
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ConditionalData {
    pub predicates: Vec<Predicate>,
    pub data: Vec<DataItem>,
}

#[derive(Clone, Debug)]
pub(crate) struct DataItem {
    pub key: String,
    pub value: Expression,
}

#[derive(Debug)]
pub enum CompileError {
    InvalidRoutePredicate { action_set: String, error: String },
    InvalidActionPredicate { service: String, error: String },
    InvalidConditionalPredicate(String),
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
            CompileError::InvalidConditionalPredicate(msg) => {
                write!(f, "Invalid conditional predicate: {}", msg)
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
                        Action::compile(action, services, id, dependencies)
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

        let tracing_enabled = self
            .actions
            .iter()
            .any(|action| matches!(action.service, ServiceInstance::Tracing(Some(_))));

        for action in &self.actions {
            let abort_on_failure =
                action.service.failure_mode() == configuration::FailureMode::Deny;

            match &action.service {
                ServiceInstance::Auth(auth_service) => {
                    let task = Box::new(AuthTask::new_with_attributes(
                        ctx,
                        action.id.clone(),
                        Rc::clone(auth_service),
                        action.scope.clone(),
                        action.predicates.clone(),
                        action.dependencies.clone(),
                        true, // pauses_filter = true for auth tasks
                    ));
                    let task = Box::new(FailureModeTask::new(task, abort_on_failure));
                    if tracing_enabled {
                        tasks.push(Box::new(TracingDecoratorTask::new(
                            "auth",
                            task,
                            action.sources.clone(),
                        )));
                    } else {
                        tasks.push(task);
                    }
                }
                ServiceInstance::RateLimit(dynamic_service)
                | ServiceInstance::RateLimitCheck(dynamic_service) => {
                    let task = Box::new(RateLimitTask::new_with_attributes(
                        ctx,
                        action.id.clone(),
                        action.dependencies.clone(),
                        Rc::clone(dynamic_service),
                        action.scope.clone(),
                        action.predicates.clone(),
                        action.conditional_data.clone(),
                        true, // pauses_filter = true for regular ratelimit and check tasks
                    ));
                    let task = Box::new(FailureModeTask::new(task, abort_on_failure));
                    if tracing_enabled {
                        tasks.push(Box::new(TracingDecoratorTask::new(
                            "ratelimit",
                            task,
                            action.sources.clone(),
                        )));
                    } else {
                        tasks.push(task);
                    }
                }
                ServiceInstance::RateLimitReport(dynamic_service) => {
                    // parse token usage from response
                    let task = Box::new(RateLimitTask::new_with_attributes(
                        ctx,
                        action.id.clone(),
                        action.dependencies.clone(),
                        Rc::clone(dynamic_service),
                        action.scope.clone(),
                        action.predicates.clone(),
                        action.conditional_data.clone(),
                        false, // pauses_filter = false for ratelimit report tasks
                    ));
                    let task = Box::new(FailureModeTask::new(task, abort_on_failure));

                    tasks.push(Box::new(TokenUsageTask::with_expected_fields(
                        action.collect_body_values(request_data),
                    )));

                    if tracing_enabled {
                        tasks.push(Box::new(TracingDecoratorTask::new(
                            "ratelimit_report",
                            task,
                            action.sources.clone(),
                        )));
                    } else {
                        tasks.push(task);
                    }
                }
                ServiceInstance::Tracing(service) => {
                    ctx.set_public_tracker_id(action.scope.clone());
                    tasks.push(Box::new(ModifyHeadersTask::new(
                        HeaderOperation::Append(
                            vec![(action.scope.clone(), ctx.request_id().to_string())].into(),
                        ),
                        HeadersType::HttpResponseHeaders,
                    )));
                    if let Some(service) = service {
                        teardown_tasks
                            .push(Box::new(ExportTracesTask::new(ctx, Rc::clone(service))));
                    }
                }
                ServiceInstance::Dynamic(dynamic_service) => {
                    let message_builder = match &action.message_builder {
                        Some(mb) => mb.clone(),
                        None => {
                            tracing::error!("Dynamic action missing message_builder");
                            continue;
                        }
                    };
                    let task: Box<dyn Task> = Box::new(DynamicTask::new(
                        action.id.clone(),
                        Rc::clone(dynamic_service),
                        action.scope.clone(),
                        message_builder,
                        action.on_reply.clone(),
                        action.predicates.clone(),
                        action.dependencies.clone(),
                    ));
                    let task = Box::new(FailureModeTask::new(task, abort_on_failure));
                    if tracing_enabled {
                        tasks.push(Box::new(TracingDecoratorTask::new(
                            "dynamic",
                            task,
                            action.sources.clone(),
                        )));
                    } else {
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
        config: &configuration::Action,
        services: &HashMap<String, ServiceInstance>,
        id: String,
        dependencies: Vec<String>,
    ) -> Result<Self, CompileError> {
        let service = services
            .get(&config.service)
            .ok_or_else(|| CompileError::UnknownService(config.service.clone()))?;

        let predicates: Vec<Predicate> = config
            .predicates
            .iter()
            .map(|p| Predicate::new(p))
            .collect::<Result<_, _>>()
            .map_err(|e| CompileError::InvalidActionPredicate {
                service: config.service.clone(),
                error: e.to_string(),
            })?;

        let conditional_data: Vec<ConditionalData> = config
            .conditional_data
            .iter()
            .map(ConditionalData::compile)
            .collect::<Result<_, _>>()?;

        Ok(Self {
            id,
            service: service.clone(),
            scope: config.scope.clone(),
            predicates,
            conditional_data,
            dependencies,
            sources: config.sources.clone(),
            message_builder: None,
            on_reply: vec![],
        })
    }

    fn compile_typed(
        typed: &configuration::TypedAction,
        services: &HashMap<String, ServiceInstance>,
        id: String,
        dependencies: Vec<String>,
    ) -> Result<Self, CompileError> {
        match &typed.operation {
            configuration::Operation::Grpc(grpc) => {
                let service_instance = services
                    .get(&grpc.service)
                    .ok_or_else(|| CompileError::UnknownService(grpc.service.clone()))?;

                if !matches!(service_instance, ServiceInstance::Dynamic(_)) {
                    return Err(CompileError::ServiceCreationFailed(format!(
                        "Service '{}' is not a dynamic service type",
                        grpc.service
                    )));
                }

                let predicate = Predicate::new(&typed.predicate).map_err(|e| {
                    CompileError::InvalidActionPredicate {
                        service: grpc.service.clone(),
                        error: e.to_string(),
                    }
                })?;

                let on_reply: Vec<TypedAction> = grpc
                    .on_reply
                    .iter()
                    .map(TypedAction::compile)
                    .collect::<Result<_, _>>()?;

                let message_builder =
                    Some(Expression::new(&grpc.message_builder).map_err(|e| {
                        CompileError::InvalidDataExpression(format!(
                            "Failed to compile message_builder: {e}"
                        ))
                    })?);

                Ok(Self {
                    id,
                    service: service_instance.clone(),
                    scope: grpc.name.clone(),
                    predicates: vec![predicate],
                    conditional_data: vec![],
                    dependencies,
                    sources: vec![],
                    message_builder,
                    on_reply,
                })
            }
            _ => Err(CompileError::InvalidDataExpression(
                "Only gRPC typed actions are currently supported as top-level actions".to_string(),
            )),
        }
    }
}

impl TypedAction {
    fn compile(config: &configuration::TypedAction) -> Result<Self, CompileError> {
        let predicate = Predicate::new(&config.predicate).map_err(|e| {
            CompileError::InvalidActionPredicate {
                service: match &config.operation {
                    configuration::Operation::Deny(_) => "deny",
                    configuration::Operation::Headers(_) => "headers",
                    configuration::Operation::Store(_) => "store",
                    configuration::Operation::Grpc(_) => "grpc",
                }
                .to_string(),
                error: e.to_string(),
            }
        })?;

        let operation = match &config.operation {
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
                let data = store
                    .data
                    .iter()
                    .map(|item| Ok((item.path.clone(), Expression::new(&item.value)?)))
                    .collect::<Result<_, CompileError>>()?;
                Operation::Store { data }
            }
            configuration::Operation::Grpc(_) => {
                return Err(CompileError::InvalidDataExpression(
                    "gRPC actions cannot be nested inside 'onReply' blocks".to_string(),
                ));
            }
        };

        Ok(TypedAction {
            predicate,
            terminal: config.terminal,
            operation,
        })
    }
}

impl ConditionalData {
    fn compile(config: &configuration::ConditionalData) -> Result<Self, CompileError> {
        let predicates: Vec<Predicate> = config
            .predicates
            .iter()
            .map(|p| Predicate::new(p))
            .collect::<Result<_, _>>()
            .map_err(|e| CompileError::InvalidConditionalPredicate(e.to_string()))?;

        let data: Vec<DataItem> = config
            .data
            .iter()
            .map(DataItem::compile)
            .collect::<Result<_, _>>()?;

        Ok(Self { predicates, data })
    }
}

impl DataItem {
    fn compile(config: &configuration::DataItem) -> Result<Self, CompileError> {
        let (key, value) = match &config.item {
            configuration::DataType::Static(s) => {
                let expr = Expression::new(&format!("'{}'", s.value))?;
                (s.key.clone(), expr)
            }
            configuration::DataType::Expression(e) => {
                let expr = Expression::new(&e.value)?;
                (e.key.clone(), expr)
            }
        };

        Ok(Self { key, value })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configuration::FailureMode;
    use crate::configuration::{
        Action as ConfigAction, ActionConfig, ActionSet, ConditionalData as ConfigConditionalData,
        DataItem as ConfigDataItem, DataType, DenyOperation, ExpressionItem, GrpcOperation,
        HeadersOperation, HeadersTarget, Operation as ConfigOperation, RouteRuleConditions,
        StaticItem, StoreItem, StoreOperation, TypedAction as ConfigTypedAction,
    };
    use crate::filter::DescriptorManager;
    use crate::services::{AuthService, DynamicService, ServiceInstance};
    use std::collections::HashMap;
    use std::rc::Rc;

    fn build_test_service(name: &str) -> (String, ServiceInstance) {
        (
            name.to_string(),
            ServiceInstance::Auth(Rc::new(AuthService::new(
                "test-cluster".to_string(),
                std::time::Duration::from_secs(10),
                FailureMode::Deny,
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

        let result = Action::compile(&config, &services, "0".to_string(), vec![]);
        assert!(result.is_ok());
        let action = result.unwrap();
        assert_eq!(action.id, "0");
        assert_eq!(action.scope, "test-scope");
        assert_eq!(action.predicates.len(), 2);
        assert!(action.dependencies.is_empty());
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

        let result = Action::compile(&config, &services, "0".to_string(), vec![]);
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

        let result = Action::compile(&config, &services, "0".to_string(), vec![]);
        assert!(matches!(
            result,
            Err(CompileError::UnknownService(ref service)) if service == "nonexistent-service"
        ));
    }

    #[test]
    fn conditional_data_compiles_with_valid_predicates_and_expressions() {
        let config = ConfigConditionalData {
            predicates: vec!["request.method == 'POST'".to_string()],
            data: vec![ConfigDataItem {
                item: DataType::Expression(ExpressionItem {
                    key: "user".to_string(),
                    value: "auth.identity.username".to_string(),
                }),
            }],
        };

        let result = ConditionalData::compile(&config);
        assert!(result.is_ok());
        let conditional = result.unwrap();
        assert_eq!(conditional.predicates.len(), 1);
        assert_eq!(conditional.data.len(), 1);
        assert_eq!(conditional.data[0].key, "user");
    }

    #[test]
    fn conditional_data_fails_on_invalid_predicate() {
        let config = ConfigConditionalData {
            predicates: vec!["invalid !!".to_string()],
            data: vec![],
        };

        let result = ConditionalData::compile(&config);
        assert!(matches!(
            result,
            Err(CompileError::InvalidConditionalPredicate(_))
        ));
    }

    #[test]
    fn data_item_compiles_static_value() {
        let config = ConfigDataItem {
            item: DataType::Static(StaticItem {
                key: "limit".to_string(),
                value: "50".to_string(),
            }),
        };

        let result = DataItem::compile(&config);
        assert!(result.is_ok());
        let data_item = result.unwrap();
        assert_eq!(data_item.key, "limit");
    }

    #[test]
    fn data_item_compiles_expression_value() {
        let config = ConfigDataItem {
            item: DataType::Expression(ExpressionItem {
                key: "host".to_string(),
                value: "request.host".to_string(),
            }),
        };

        let result = DataItem::compile(&config);
        assert!(result.is_ok());
        let data_item = result.unwrap();
        assert_eq!(data_item.key, "host");
    }

    #[test]
    fn data_item_fails_on_invalid_expression() {
        let config = ConfigDataItem {
            item: DataType::Expression(ExpressionItem {
                key: "test".to_string(),
                value: "bad syntax !!!".to_string(),
            }),
        };

        let result = DataItem::compile(&config);
        assert!(matches!(
            result,
            Err(CompileError::InvalidDataExpression(_))
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

        let result = Blueprint::compile(&config, &services);
        assert!(result.is_ok());
        let blueprint = result.unwrap();
        assert_eq!(blueprint.name, "complete-test");
        assert_eq!(blueprint.route_predicates.len(), 1);
        assert_eq!(blueprint.actions.len(), 1);
        assert_eq!(blueprint.actions[0].predicates.len(), 1);
        assert_eq!(blueprint.actions[0].conditional_data.len(), 1);
        assert_eq!(blueprint.actions[0].conditional_data[0].data.len(), 2);
    }

    #[test]
    fn grpc_typed_action_compiles() {
        let services = HashMap::from([build_dynamic_service("my-dynamic")]);

        let typed = ConfigTypedAction {
            predicate: "request.method == 'GET'".to_string(),
            terminal: false,
            operation: ConfigOperation::Grpc(GrpcOperation {
                name: "rl_check".to_string(),
                service: "my-dynamic".to_string(),
                message_builder: "envoy.service.ratelimit.v3.RateLimitRequest{}".to_string(),
                on_reply: vec![
                    ConfigTypedAction {
                        predicate: "rl_check.overall_code == 2".to_string(),
                        terminal: true,
                        operation: ConfigOperation::Deny(DenyOperation {
                            deny_with: "DenyResponse{status: 429u}".to_string(),
                        }),
                    },
                    ConfigTypedAction {
                        predicate: "true".to_string(),
                        terminal: false,
                        operation: ConfigOperation::Headers(HeadersOperation {
                            target: HeadersTarget::Request,
                            headers: "result.headers".to_string(),
                        }),
                    },
                    ConfigTypedAction {
                        predicate: "true".to_string(),
                        terminal: false,
                        operation: ConfigOperation::Store(StoreOperation {
                            data: vec![StoreItem {
                                path: "rl.remaining".to_string(),
                                value: "result.remaining".to_string(),
                            }],
                        }),
                    },
                ],
            }),
        };

        let result = Action::compile_typed(&typed, &services, "0".to_string(), vec![]);
        assert!(result.is_ok());
        let action = result.unwrap();
        assert_eq!(action.id, "0");
        assert_eq!(action.scope, "rl_check");
        assert!(matches!(action.service, ServiceInstance::Dynamic(_)));
        assert_eq!(action.predicates.len(), 1);
        assert!(action.message_builder.is_some());
        assert_eq!(action.on_reply.len(), 3);
        assert!(action.conditional_data.is_empty());
    }

    #[test]
    fn grpc_typed_action_fails_on_unknown_service() {
        let services = HashMap::new();

        let typed = ConfigTypedAction {
            predicate: "true".to_string(),
            terminal: false,
            operation: ConfigOperation::Grpc(GrpcOperation {
                name: "check".to_string(),
                service: "nonexistent".to_string(),
                message_builder: "test.Request{}".to_string(),
                on_reply: vec![],
            }),
        };

        let result = Action::compile_typed(&typed, &services, "0".to_string(), vec![]);
        assert!(matches!(result, Err(CompileError::UnknownService(ref s)) if s == "nonexistent"));
    }

    #[test]
    fn grpc_typed_action_fails_on_non_dynamic_service() {
        let services = HashMap::from([build_test_service("auth-svc")]);

        let typed = ConfigTypedAction {
            predicate: "true".to_string(),
            terminal: false,
            operation: ConfigOperation::Grpc(GrpcOperation {
                name: "check".to_string(),
                service: "auth-svc".to_string(),
                message_builder: "test.Request{}".to_string(),
                on_reply: vec![],
            }),
        };

        let result = Action::compile_typed(&typed, &services, "0".to_string(), vec![]);
        assert!(matches!(
            result,
            Err(CompileError::ServiceCreationFailed(_))
        ));
    }

    #[test]
    fn grpc_in_on_reply_block_is_rejected() {
        let nested_grpc = ConfigTypedAction {
            predicate: "true".to_string(),
            terminal: false,
            operation: ConfigOperation::Grpc(GrpcOperation {
                name: "nested".to_string(),
                service: "svc".to_string(),
                message_builder: "test.Request{}".to_string(),
                on_reply: vec![],
            }),
        };

        let result = TypedAction::compile(&nested_grpc);
        assert!(matches!(
            result,
            Err(CompileError::InvalidDataExpression(ref msg)) if msg.contains("cannot be nested")
        ));
    }

    #[test]
    fn typed_actions_compile() {
        let deny_config = ConfigTypedAction {
            predicate: "result.code == 2".to_string(),
            terminal: true,
            operation: ConfigOperation::Deny(DenyOperation {
                deny_with: "DenyResponse{status: 429u}".to_string(),
            }),
        };
        let deny_result = TypedAction::compile(&deny_config);
        assert!(deny_result.is_ok());
        let deny = deny_result.unwrap();
        assert!(deny.terminal);
        assert!(matches!(deny.operation, Operation::Deny { .. }));

        let headers_config = ConfigTypedAction {
            predicate: "true".to_string(),
            terminal: false,
            operation: ConfigOperation::Headers(HeadersOperation {
                target: HeadersTarget::Response,
                headers: "result.resp_headers".to_string(),
            }),
        };
        let headers_result = TypedAction::compile(&headers_config);
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
            operation: ConfigOperation::Store(StoreOperation {
                data: vec![
                    StoreItem {
                        path: "a.b".to_string(),
                        value: "result.x".to_string(),
                    },
                    StoreItem {
                        path: "c.d".to_string(),
                        value: "result.y".to_string(),
                    },
                ],
            }),
        };
        let store_result = TypedAction::compile(&store_config);
        assert!(store_result.is_ok());
        let store = store_result.unwrap();
        assert!(!store.terminal);
        assert!(matches!(
            store.operation,
            Operation::Store { ref data, .. }
                if data.len() == 2
                && data[0].0 == "a.b"
        ));
    }

    #[test]
    fn typed_action_fails_on_invalid_predicate() {
        let config = ConfigTypedAction {
            predicate: "bad syntax !!".to_string(),
            terminal: true,
            operation: ConfigOperation::Deny(DenyOperation {
                deny_with: "DenyResponse{status: 429u}".to_string(),
            }),
        };
        let result = TypedAction::compile(&config);
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
                    operation: ConfigOperation::Grpc(GrpcOperation {
                        name: "rl_check".to_string(),
                        service: "dyn-svc".to_string(),
                        message_builder: "test.Request{}".to_string(),
                        on_reply: vec![ConfigTypedAction {
                            predicate: "rl_check.code == 2".to_string(),
                            terminal: true,
                            operation: ConfigOperation::Deny(DenyOperation {
                                deny_with: "DenyResponse{status: 429u}".to_string(),
                            }),
                        }],
                    }),
                }),
            ],
        };

        let result = Blueprint::compile(&config, &services);
        assert!(result.is_ok());
        let blueprint = result.unwrap();
        assert_eq!(blueprint.actions.len(), 2);

        assert!(matches!(
            blueprint.actions[0].service,
            ServiceInstance::Auth(_)
        ));
        assert!(blueprint.actions[0].message_builder.is_none());
        assert!(blueprint.actions[0].on_reply.is_empty());
        assert!(blueprint.actions[0].dependencies.is_empty());

        assert!(matches!(
            blueprint.actions[1].service,
            ServiceInstance::Dynamic(_)
        ));
        assert!(blueprint.actions[1].message_builder.is_some());
        assert_eq!(blueprint.actions[1].on_reply.len(), 1);
        assert_eq!(blueprint.actions[1].dependencies, vec!["0"]);
    }
}
