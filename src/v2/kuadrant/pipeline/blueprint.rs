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
