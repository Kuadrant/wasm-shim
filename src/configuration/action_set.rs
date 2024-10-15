use crate::configuration::action::Action;
use crate::configuration::PatternExpression;
use log::debug;
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
                .all(|m| self.pattern_expression_applies(m))
    }

    fn pattern_expression_applies(&self, p_e: &PatternExpression) -> bool {
        let attribute_path = p_e.path();

        let attribute_value = match crate::property::get_property(attribute_path).unwrap() {
            //TODO(didierofrivia): Replace hostcalls by DI
            None => {
                debug!(
                    "pattern_expression_applies:  selector not found: {}, defaulting to ``",
                    p_e.selector
                );
                b"".to_vec()
            }
            Some(attribute_bytes) => attribute_bytes,
        };
        p_e.eval(attribute_value).unwrap_or_else(|e| {
            debug!("pattern_expression_applies failed: {}", e);
            false
        })
    }
}
