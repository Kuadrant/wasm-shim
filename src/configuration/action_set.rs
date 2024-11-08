use crate::configuration::action::Action;
use crate::data::Predicate;
use log::error;
use serde::Deserialize;
use std::cell::OnceCell;

#[derive(Deserialize, Debug, Clone, Default)]
pub struct RouteRuleConditions {
    pub hostnames: Vec<String>,
    #[serde(default)]
    pub predicates: Vec<String>,
    #[serde(skip_deserializing)]
    pub compiled_predicates: OnceCell<Vec<Predicate>>,
}

#[derive(Default, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ActionSet {
    pub name: String,
    pub route_rule_conditions: RouteRuleConditions,
    pub actions: Vec<Action>,
}

impl ActionSet {
    #[cfg(test)]
    pub fn new(
        name: String,
        route_rule_conditions: RouteRuleConditions,
        actions: Vec<Action>,
    ) -> Self {
        ActionSet {
            name,
            route_rule_conditions,
            actions,
        }
    }

    pub fn conditions_apply(&self) -> bool {
        let predicates = self
            .route_rule_conditions
            .compiled_predicates
            .get()
            .expect("predicates must be compiled by now");
        predicates.is_empty()
            || predicates
                .iter()
                .enumerate()
                .all(|(pos, predicate)| match predicate.test() {
                    Ok(b) => b,
                    Err(err) => {
                        error!(
                            "Failed to evaluate {}: {}",
                            self.route_rule_conditions.predicates[pos], err
                        );
                        panic!("Err out of this!")
                    }
                })
    }
}

#[cfg(test)]
mod test {
    use crate::configuration::action_set::ActionSet;

    fn build_action_set(name: &str) -> ActionSet {
        ActionSet::new(name.to_owned(), Default::default(), Vec::new())
    }

    #[test]
    fn empty_route_rule_conditions_do_apply() {
        let action_set_1 = build_action_set("as_1");
        action_set_1
            .route_rule_conditions
            .compiled_predicates
            .set(Vec::default())
            .expect("Predicates must not be compiled yet!");

        assert!(action_set_1.conditions_apply())
    }
}
