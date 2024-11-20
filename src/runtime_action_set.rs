use crate::configuration::{ActionSet, Service};
use crate::data::Predicate;
use crate::runtime_action::RuntimeAction;
use log::error;
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
    ) -> Result<Self, String> {
        // route predicates
        let mut route_rule_predicates = Vec::default();
        for predicate in &action_set.route_rule_conditions.predicates {
            route_rule_predicates
                .push(Predicate::route_rule(predicate).map_err(|e| e.to_string())?);
        }

        // actions
        let mut runtime_actions = Vec::default();
        for action in &action_set.actions {
            runtime_actions.push(Rc::new(RuntimeAction::new(action, services)?));
        }

        Ok(Self {
            name: action_set.name.clone(),
            route_rule_predicates,
            runtime_actions,
        })
    }

    pub fn conditions_apply(&self) -> bool {
        let predicates = &self.route_rule_predicates;
        predicates.is_empty()
            || predicates.iter().all(|predicate| match predicate.test() {
                Ok(b) => b,
                Err(err) => {
                    error!("Failed to evaluate {:?}: {}", predicate, err);
                    panic!("Err out of this!")
                }
            })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::configuration::ActionSet;

    #[test]
    fn empty_route_rule_predicates_do_apply() {
        let action_set = ActionSet::new("some_name".to_owned(), Default::default(), Vec::new());

        let runtime_action_set = RuntimeActionSet::new(&action_set, &HashMap::default())
            .expect("should not happen from an empty set of actions");

        assert!(runtime_action_set.conditions_apply())
    }
}
