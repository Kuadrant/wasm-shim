use crate::configuration::{Action, PatternExpression};
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
pub struct Rule {
    pub conditions: Vec<PatternExpression>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Policy {
    pub name: String,
    pub hostnames: Vec<String>,
    pub rules: Vec<Rule>,
    pub actions: Vec<Action>,
}

impl Policy {
    #[cfg(test)]
    pub fn new(
        name: String,
        hostnames: Vec<String>,
        rules: Vec<Rule>,
        actions: Vec<Action>,
    ) -> Self {
        Policy {
            name,
            hostnames,
            rules,
            actions,
        }
    }
}
