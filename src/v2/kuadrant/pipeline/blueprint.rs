use crate::configuration;
use crate::data::{cel::Predicate, Expression};
use crate::v2::kuadrant::pipeline::tasks::{
    AuthTask, FailureModeTask, RateLimitTask, Task, TokenUsageTask,
};
use crate::v2::kuadrant::ReqRespCtx;
use crate::services::ServiceInstance;
use cel_parser::ParseError;
use std::collections::HashMap;
use std::fmt::Display;
use std::rc::Rc;

pub(super) struct Blueprint {
    pub name: String,
    pub route_predicates: Vec<Predicate>,
    pub actions: Vec<Action>,
}

pub(super) struct Action {
    pub id: String,
    pub service: ServiceInstance,
    pub scope: String,
    pub predicates: Vec<Predicate>,
    pub conditional_data: Vec<ConditionalData>,
    pub dependencies: Vec<String>,
}

#[derive(Clone)]
pub(super) struct ConditionalData {
    pub predicates: Vec<Predicate>,
    pub data: Vec<DataItem>,
}

#[derive(Clone)]
pub(super) struct DataItem {
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

impl From<ParseError> for CompileError {
    fn from(e: ParseError) -> Self {
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
            .map(|(i, action)| {
                let id = i.to_string();
                let dependencies = if i > 0 {
                    vec![(i - 1).to_string()]
                } else {
                    vec![]
                };
                Action::compile(action, services, id, dependencies)
            })
            .collect::<Result<_, _>>()?;

        Ok(Self {
            name: config.name.clone(),
            route_predicates,
            actions,
        })
    }
}

impl Blueprint {
    pub fn to_tasks(&self, ctx: &ReqRespCtx) -> Vec<Box<dyn Task>> {
        let mut tasks: Vec<Box<dyn Task>> = Vec::new();

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
                    tasks.push(Box::new(FailureModeTask::new(task, abort_on_failure)));
                }
                ServiceInstance::RateLimit(ratelimit_service)
                | ServiceInstance::RateLimitCheck(ratelimit_service) => {
                    let task = Box::new(RateLimitTask::new_with_attributes(
                        ctx,
                        action.id.clone(),
                        action.dependencies.clone(),
                        Rc::clone(ratelimit_service),
                        action.scope.clone(),
                        action.predicates.clone(),
                        action.conditional_data.clone(),
                        true, // pauses_filter = true for regular ratelimit and check tasks
                    ));
                    tasks.push(Box::new(FailureModeTask::new(task, abort_on_failure)));
                }
                ServiceInstance::RateLimitReport(ratelimit_service) => {
                    // parse token usage from response
                    tasks.push(Box::new(TokenUsageTask::new()));
                    let task = Box::new(RateLimitTask::new_with_attributes(
                        ctx,
                        action.id.clone(),
                        action.dependencies.clone(),
                        Rc::clone(ratelimit_service),
                        action.scope.clone(),
                        action.predicates.clone(),
                        action.conditional_data.clone(),
                        false, // pauses_filter = false for ratelimit report tasks
                    ));
                    tasks.push(Box::new(FailureModeTask::new(task, abort_on_failure)));
                }
            }
        }

        tasks
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
        Action as ConfigAction, ActionSet, ConditionalData as ConfigConditionalData,
        DataItem as ConfigDataItem, DataType, ExpressionItem, RouteRuleConditions, StaticItem,
    };
    use crate::services::{AuthService, ServiceInstance};
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
            actions: vec![ConfigAction {
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
            }],
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
}
