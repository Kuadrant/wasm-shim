use crate::configuration::action::Action;
use crate::configuration::PatternExpression;
use crate::data::Predicate;
use log::error;
use serde::Deserialize;
use std::cell::OnceCell;

#[derive(Deserialize, Debug, Clone, Default)]
pub struct RouteRuleConditions {
    pub hostnames: Vec<String>,
    #[serde(default)]
    pub matches: Vec<PatternExpression>,
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
        if predicates.is_empty() {
            self.route_rule_conditions.matches.is_empty()
                || self
                    .route_rule_conditions
                    .matches
                    .iter()
                    .all(|m| m.applies())
        } else {
            predicates
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
}
