use crate::envoy::RLA_action_specifier;
use crate::policy_index::PolicyIndex;
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
pub struct Rule {
    pub paths: Option<Vec<String>>,
    pub hosts: Option<Vec<String>>,
    pub methods: Option<Vec<String>>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Configuration {
    pub actions: Option<Vec<RLA_action_specifier>>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct GatewayAction {
    pub rules: Option<Vec<Rule>>,
    pub configurations: Option<Vec<Configuration>>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct RateLimitPolicy {
    pub name: String,
    pub rate_limit_domain: String,
    pub upstream_cluster: String,
    pub hostnames: Vec<String>,
    pub gateway_actions: Vec<GatewayAction>,
}

impl RateLimitPolicy {
    #[cfg(test)]
    pub fn new(
        name: String,
        rate_limit_domain: String,
        upstream_cluster: String,
        hostnames: Vec<String>,
        gateway_actions: Vec<GatewayAction>,
    ) -> Self {
        RateLimitPolicy {
            name,
            rate_limit_domain,
            upstream_cluster,
            hostnames,
            gateway_actions,
        }
    }
}

pub struct FilterConfig {
    pub index: PolicyIndex,
    // Deny request when faced with an irrecoverable failure.
    pub failure_mode_deny: bool,
}

impl FilterConfig {
    pub fn new() -> Self {
        Self {
            index: PolicyIndex::new(),
            failure_mode_deny: true,
        }
    }

    pub fn from(config: PluginConfiguration) -> Self {
        let mut index = PolicyIndex::new();

        for rlp in config.rate_limit_policies.iter() {
            for hostname in rlp.hostnames.iter() {
                index.insert(hostname, rlp.clone());
            }
        }

        Self {
            index,
            failure_mode_deny: config.failure_mode_deny,
        }
    }
}

// TODO(rahulanand16nov): We can convert the structure of config in such a way
// that it's optimized for lookup in the runtime. For e.g., keying on virtualhost
// to sort through ratelimitpolicies and then further operations.

#[derive(Deserialize, Debug, Clone)]
pub struct PluginConfiguration {
    pub rate_limit_policies: Vec<RateLimitPolicy>,
    // Deny request when faced with an irrecoverable failure.
    pub failure_mode_deny: bool,
}

#[cfg(test)]
mod test {
    use super::*;

    const CONFIG: &str = r#"{
        "failure_mode_deny": true,
        "rate_limit_policies": [
        {
            "name": "some-name",
            "rate_limit_domain": "RLS-domain",
            "upstream_cluster": "limitador-cluster",
            "hostnames": ["*.toystore.com", "example.com"],
            "gateway_actions": [
            {
                "rules": [
                {
                    "paths": ["/admin/toy"],
                    "hosts": ["cars.toystore.com"],
                    "methods": ["POST"]
                }],
                "configurations": [
                {
                    "actions": [
                    {
                        "generic_key": {
                            "descriptor_key": "admin",
                            "descriptor_value": "1"
                        }
                    }
                    ]
                }
                ]
            }
            ]
        }
        ]
    }"#;

    #[test]
    fn parse_config() {
        let res = serde_json::from_str::<PluginConfiguration>(CONFIG);
        if let Err(ref e) = res {
            eprintln!("{}", e);
        }
        assert!(res.is_ok());
    }

    #[test]
    fn filter_config_from_configuration() {
        let res = serde_json::from_str::<PluginConfiguration>(CONFIG);
        assert!(res.is_ok());

        let filter_config = FilterConfig::from(res.unwrap());
        let rlp_option = filter_config.index.get_longest_match_policy("example.com");
        assert!(rlp_option.is_some());

        let rlp_option = filter_config
            .index
            .get_longest_match_policy("test.toystore.com");
        assert!(rlp_option.is_some());

        let rlp_option = filter_config.index.get_longest_match_policy("unknown");
        assert!(rlp_option.is_none());
    }
}
