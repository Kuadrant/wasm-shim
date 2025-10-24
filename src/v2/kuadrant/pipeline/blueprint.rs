use crate::v2::configuration;
use crate::v2::data::Expression;
use cel_parser::ParseError;
use std::collections::HashMap;
use std::rc::Rc;

pub(super) struct Blueprint {
    pub name: String,
    pub route_predicates: Vec<Expression>,
    pub actions: Vec<Action>,
}

pub(super) struct Action {
    pub service: Rc<configuration::Service>,
    pub scope: String,
    pub predicates: Vec<Expression>,
    pub conditional_data: Vec<ConditionalData>,
}

pub(super) struct ConditionalData {
    pub predicates: Vec<Expression>,
    pub data: Vec<DataItem>,
}

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
}

impl From<ParseError> for CompileError {
    fn from(e: ParseError) -> Self {
        CompileError::InvalidDataExpression(e.to_string())
    }
}

impl Blueprint {
    pub fn compile(
        config: &configuration::ActionSet,
        services: &HashMap<String, Rc<configuration::Service>>,
    ) -> Result<Self, CompileError> {
        let route_predicates: Vec<Expression> = config
            .route_rule_conditions
            .predicates
            .iter()
            .map(|p| Expression::new_extended(p))
            .collect::<Result<_, _>>()
            .map_err(|e| CompileError::InvalidRoutePredicate {
                action_set: config.name.clone(),
                error: e.to_string(),
            })?;

        let actions: Vec<Action> = config
            .actions
            .iter()
            .map(|action| Action::compile(action, services))
            .collect::<Result<_, _>>()?;

        Ok(Self {
            name: config.name.clone(),
            route_predicates,
            actions,
        })
    }
}

impl Action {
    fn compile(
        config: &configuration::Action,
        services: &HashMap<String, Rc<configuration::Service>>,
    ) -> Result<Self, CompileError> {
        let service = services
            .get(&config.service)
            .ok_or_else(|| CompileError::UnknownService(config.service.clone()))?;

        let predicates: Vec<Expression> = config
            .predicates
            .iter()
            .map(|p| Expression::new(p))
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
            service: Rc::clone(service),
            scope: config.scope.clone(),
            predicates,
            conditional_data,
        })
    }
}

impl ConditionalData {
    fn compile(config: &configuration::ConditionalData) -> Result<Self, CompileError> {
        let predicates: Vec<Expression> = config
            .predicates
            .iter()
            .map(|p| Expression::new(p))
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
    use crate::v2::configuration::{
        Action as ConfigAction, ActionSet, ConditionalData as ConfigConditionalData,
        DataItem as ConfigDataItem, DataType, ExpressionItem, FailureMode, RouteRuleConditions,
        Service, ServiceType, StaticItem, Timeout,
    };

    fn build_test_service(name: &str) -> (String, Rc<Service>) {
        (
            name.to_string(),
            Rc::new(Service {
                service_type: ServiceType::Auth,
                endpoint: "test-cluster".to_string(),
                failure_mode: FailureMode::Deny,
                timeout: Timeout::default(),
            }),
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

        let result = Action::compile(&config, &services);
        assert!(result.is_ok());
        let action = result.unwrap();
        assert_eq!(action.scope, "test-scope");
        assert_eq!(action.predicates.len(), 2);
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

        let result = Action::compile(&config, &services);
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

        let result = Action::compile(&config, &services);
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
