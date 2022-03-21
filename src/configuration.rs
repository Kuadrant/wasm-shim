use crate::envoy::RLA_action_specifier;
use crate::glob::GlobPatternSet;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize, Debug, Clone)]
pub struct Operation {
    #[serde(default)]
    pub paths: GlobPatternSet,
    #[serde(default)]
    pub hosts: GlobPatternSet,
    #[serde(default)]
    pub methods: GlobPatternSet,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Rule {
    pub operations: Option<Vec<Operation>>,
    pub actions: Option<Vec<RLA_action_specifier>>,
}

impl Rule {
    pub fn operations(&self) -> Option<&[Operation]> {
        self.operations.as_deref()
    }

    pub fn actions(&self) -> Option<&[RLA_action_specifier]> {
        self.actions.as_deref()
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct RateLimitPolicy {
    rules: Option<Vec<Rule>>,
    global_actions: Option<Vec<RLA_action_specifier>>,
    upstream_cluster: Option<String>,
    domain: Option<String>,
}

impl RateLimitPolicy {
    pub fn rules(&self) -> Option<&[Rule]> {
        self.rules.as_deref()
    }

    pub fn global_actions(&self) -> Option<&[RLA_action_specifier]> {
        self.global_actions.as_deref()
    }

    pub fn upstream_cluster(&self) -> Option<&str> {
        self.upstream_cluster.as_deref()
    }

    pub fn domain(&self) -> Option<&str> {
        self.domain.as_deref()
    }
}

// TODO(rahulanand16nov): We can convert the structure of config in such a way
// that it's optimized for lookup in the runtime. For e.g., keying on virtualhost
// to sort through ratelimitpolicies and then further operations.

#[derive(Deserialize, Debug, Clone)]
pub struct FilterConfig {
    ratelimitpolicies: HashMap<String, RateLimitPolicy>,
    // Deny request when faced with an irrecoverable failure.
    failure_mode_deny: bool,
}

impl FilterConfig {
    pub fn new() -> Self {
        FilterConfig {
            ratelimitpolicies: HashMap::new(),
            failure_mode_deny: true,
        }
    }

    pub fn ratelimitpolicies(&self) -> &HashMap<String, RateLimitPolicy> {
        &self.ratelimitpolicies
    }

    pub fn failure_mode_deny(&self) -> bool {
        self.failure_mode_deny
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn parse_config() {
        const CONFIG: &str = r#"{
            "failure_mode_deny": true,
            "ratelimitpolicies": {
                "default-toystore": {
                    "rules": [{
                        "operations": [{
                            "paths": ["/toy*"],
                            "hosts": ["*.toystore.com"],
                            "methods": ["GET"]
                        }],
                        "actions": [{
                            "generic_key": {
                                "descriptor_value": "yes",
                                "descriptor_key": "get-toy"
                            }
                        }]
                    }],
                    "global_actions": [{
                        "generic_key": {
                            "descriptor_value": "yes",
                            "descriptor_key": "vhost-level"
                        }
                    }],
                    "upstream_cluster": "outbound|8080||limitador.kuadrant-system.svc.cluster.local",
                    "domain": "toystore"
                }
            }
        }"#;

        let res = serde_json::from_str::<FilterConfig>(CONFIG);
        if let Err(ref e) = res {
            eprintln!("{}", e);
        }
        assert!(res.is_ok());
    }
}
