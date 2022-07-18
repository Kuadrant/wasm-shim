use crate::envoy::RLA_action_specifier;
use crate::policy_index::PolicyIndex;
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
pub struct Rule {
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub methods: Vec<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Configuration {
    #[serde(default)]
    pub actions: Vec<RLA_action_specifier>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct GatewayAction {
    #[serde(default)]
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub configurations: Vec<Configuration>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct RateLimitPolicy {
    pub name: String,
    pub rate_limit_domain: String,
    pub upstream_cluster: String,
    pub hostnames: Vec<String>,
    #[serde(default)]
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
                    },
                    {
                        "metadata": {
                            "descriptor_key": "user-id",
                            "default_value": "no-user",
                            "metadata_key": {
                                "key": "envoy.filters.http.ext_authz",
                                "path": [
                                    {
                                        "segment": {
                                            "key": "ext_auth_data"
                                        }
                                    },
                                    {
                                        "segment": {
                                            "key": "user_id"
                                        }
                                    }
                                ]
                            },
                            "source": "DYNAMIC"
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

        let filter_config = res.unwrap();
        assert_eq!(filter_config.rate_limit_policies.len(), 1);

        let gateway_actions = &filter_config.rate_limit_policies[0].gateway_actions;
        assert_eq!(gateway_actions.len(), 1);

        let configurations = &gateway_actions[0].configurations;
        assert_eq!(configurations.len(), 1);

        let actions = &configurations[0].actions;
        assert_eq!(actions.len(), 2);
        assert!(std::matches!(
            actions[0],
            RLA_action_specifier::generic_key(_)
        ));

        if let RLA_action_specifier::metadata(ref metadata_action) = actions[1] {
            let metadata_key = metadata_action.get_metadata_key();
            assert_eq!(metadata_key.get_key(), "envoy.filters.http.ext_authz");

            let metadata_path = metadata_key.get_path();
            assert_eq!(metadata_path.len(), 2);
            assert_eq!(metadata_path[0].get_key(), "ext_auth_data");
            assert_eq!(metadata_path[1].get_key(), "user_id");
        } else {
            panic!("wrong action type: expected metadata type");
        }
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
