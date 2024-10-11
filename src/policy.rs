use crate::configuration::{Action, PatternExpression};
use log::debug;
use proxy_wasm::hostcalls;
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Condition {
    pub all_of: Vec<PatternExpression>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Rule {
    #[serde(default)]
    pub conditions: Vec<Condition>,
    pub actions: Vec<Action>,
}

#[derive(Default, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Policy {
    pub name: String,
    pub hostnames: Vec<String>,
    pub rules: Vec<Rule>,
}

impl Policy {
    #[cfg(test)]
    pub fn new(name: String, hostnames: Vec<String>, rules: Vec<Rule>) -> Self {
        Policy {
            name,
            hostnames,
            rules,
        }
    }

    pub fn find_rule_that_applies(&self) -> Option<&Rule> {
        self.rules
            .iter()
            .find(|rule: &&Rule| self.filter_rule_by_conditions(&rule.conditions))
    }

    fn filter_rule_by_conditions(&self, conditions: &[Condition]) -> bool {
        if conditions.is_empty() {
            // no conditions is equivalent to matching all the requests.
            return true;
        }

        conditions
            .iter()
            .any(|condition| self.condition_applies(condition))
    }

    fn condition_applies(&self, condition: &Condition) -> bool {
        condition
            .all_of
            .iter()
            .all(|pattern_expression| self.pattern_expression_applies(pattern_expression))
    }

    fn pattern_expression_applies(&self, p_e: &PatternExpression) -> bool {
        let attribute_path = p_e.path();
        debug!(
            "get_property:  selector: {} path: {:?}",
            p_e.selector, attribute_path
        );
        let attribute_value = match hostcalls::get_property(attribute_path).unwrap() {
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
