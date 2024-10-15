use crate::configuration::action::Action;
use crate::configuration::PatternExpression;
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone, Default)]
pub struct RouteRuleConditions {
    pub hostnames: Vec<String>,
    #[serde(default)]
    pub matches: Vec<PatternExpression>,
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
        self.route_rule_conditions.matches.is_empty()
            || self
                .route_rule_conditions
                .matches
                .iter()
                .all(|m| m.applies())
    }
}
