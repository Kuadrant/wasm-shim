use crate::configuration::{ActionSet, Service};
use crate::data::{
    Attribute, AttributeOwner, AttributeResolver, Predicate, PredicateResult, PredicateVec,
};
use crate::runtime_action::errors::ActionCreationError;
use crate::runtime_action::RuntimeAction;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Debug)]
pub struct RuntimeActionSet {
    pub name: String,
    pub route_rule_predicates: Vec<Predicate>,
    pub runtime_actions: Vec<Rc<RuntimeAction>>,
}

impl RuntimeActionSet {
    pub fn new(
        action_set: &ActionSet,
        services: &HashMap<String, Service>,
    ) -> Result<Self, ActionCreationError> {
        // route predicates
        let mut route_rule_predicates = Vec::default();
        for predicate in &action_set.route_rule_conditions.predicates {
            route_rule_predicates.push(Predicate::route_rule(predicate)?);
        }

        // actions
        let mut runtime_actions = Vec::default();
        for action in action_set.actions.iter() {
            runtime_actions.push(RuntimeAction::new(action, services)?);
        }

        Ok(Self {
            name: action_set.name.clone(),
            route_rule_predicates,
            runtime_actions: runtime_actions.into_iter().map(Rc::new).collect(),
        })
    }

    pub fn conditions_apply<T>(&self, resolver: &mut T) -> PredicateResult
    where
        T: AttributeResolver,
    {
        self.route_rule_predicates.apply(resolver)
    }
}

impl AttributeOwner for RuntimeActionSet {
    fn request_attributes(&self) -> Vec<&Attribute> {
        self.runtime_actions
            .iter()
            .flat_map(|action| action.request_attributes())
            .collect()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::configuration::{ActionSet, RouteRuleConditions};
    use crate::data::PathCache;

    #[test]
    fn empty_route_rule_predicates_do_apply() {
        let action_set = ActionSet::new("some_name".to_owned(), Default::default(), Vec::new());

        let runtime_action_set = RuntimeActionSet::new(&action_set, &HashMap::default())
            .expect("should not happen from an empty set of actions");
        assert!(runtime_action_set
            .conditions_apply(&mut PathCache::default())
            .expect("should not fail!"));
    }

    #[test]
    fn when_all_predicates_are_truthy_conditions_apply() {
        let action_set = ActionSet::new(
            "some_name".to_owned(),
            RouteRuleConditions {
                hostnames: Vec::default(),
                predicates: vec!["true".into(), "true".into()],
            },
            Vec::new(),
        );

        let runtime_action_set = RuntimeActionSet::new(&action_set, &HashMap::default())
            .expect("should not happen from an empty set of actions");
        assert!(runtime_action_set
            .conditions_apply(&mut PathCache::default())
            .expect("should not fail!"));
    }

    #[test]
    fn when_not_all_predicates_are_truthy_action_does_not_apply() {
        let action_set = ActionSet::new(
            "some_name".to_owned(),
            RouteRuleConditions {
                hostnames: Vec::default(),
                predicates: vec!["true".into(), "true".into(), "true".into(), "false".into()],
            },
            Vec::new(),
        );

        let runtime_action_set = RuntimeActionSet::new(&action_set, &HashMap::default())
            .expect("should not happen from an empty set of actions");
        assert!(!runtime_action_set
            .conditions_apply(&mut PathCache::default())
            .expect("should not fail!"));
    }

    #[test]
    fn when_a_cel_expression_does_not_evaluate_to_bool_returns_error() {
        let action_set = ActionSet::new(
            "some_name".to_owned(),
            RouteRuleConditions {
                hostnames: Vec::default(),
                predicates: vec!["true".into(), "true".into(), "true".into(), "1".into()],
            },
            Vec::new(),
        );

        let runtime_action_set = RuntimeActionSet::new(&action_set, &HashMap::default())
            .expect("should not happen from an empty set of actions");
        assert!(runtime_action_set
            .conditions_apply(&mut PathCache::default())
            .is_err());
    }
}
