use crate::action_set_index::ActionSetIndex;
use crate::configuration::PluginConfiguration;
use crate::runtime_action_set::RuntimeActionSet;
use std::rc::Rc;

pub(crate) struct RuntimeConfig {
    pub index: ActionSetIndex,
}

impl TryFrom<PluginConfiguration> for RuntimeConfig {
    type Error = String;

    fn try_from(config: PluginConfiguration) -> Result<Self, Self::Error> {
        let mut index = ActionSetIndex::new();
        for action_set in config.action_sets.iter() {
            let runtime_action_set = Rc::new(RuntimeActionSet::new(action_set, &config.services)?);
            for hostname in action_set.route_rule_conditions.hostnames.iter() {
                index.insert(hostname, Rc::clone(&runtime_action_set));
            }
        }

        Ok(Self { index })
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            index: ActionSetIndex::new(),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    const CONFIG: &str = r#"{
        "services": {
            "authorino": {
                "type": "auth",
                "endpoint": "authorino-cluster",
                "failureMode": "deny",
                "timeout": "24ms"
            },
            "limitador": {
                "type": "ratelimit",
                "endpoint": "limitador-cluster",
                "failureMode": "allow",
                "timeout": "42ms"
            }
        },
        "actionSets": [
        {
            "name": "rlp-ns-A/rlp-name-A",
            "routeRuleConditions": {
                "hostnames": ["*.toystore.com", "example.com"],
                "predicates": [
                    "request.path == '/admin/toy'",
                    "request.method == 'POST'",
                    "request.host == 'cars.toystore.com'"
                ]
            },
            "actions": [
            {
                "service": "authorino",
                "scope": "authconfig-A"
            },
            {
                "service": "limitador",
                "scope": "rlp-ns-A/rlp-name-A",
                "predicates": [
                    "auth.metadata.username == 'alice'"
                ],
                "data": [
                {
                    "static": {
                        "key": "rlp-ns-A/rlp-name-A",
                        "value": "1"
                    }
                },
                {
                    "expression": {
                        "key": "username",
                        "value": "auth.metadata.username"
                    }
                }]
            }]
        }]
    }"#;

    #[test]
    fn runtime_config_from_configuration() {
        let res = serde_json::from_str::<PluginConfiguration>(CONFIG);
        if let Err(ref e) = res {
            eprintln!("{e}");
        }
        assert!(res.is_ok());

        let result = RuntimeConfig::try_from(res.unwrap());
        let runtime_config = result.expect("That didn't work");
        let rlp_option = runtime_config
            .index
            .get_longest_match_action_sets("example.com");
        assert!(rlp_option.is_some());

        let rlp_option = runtime_config
            .index
            .get_longest_match_action_sets("test.toystore.com");
        assert!(rlp_option.is_some());

        let rlp_option = runtime_config
            .index
            .get_longest_match_action_sets("unknown");
        assert!(rlp_option.is_none());
    }

    #[test]
    fn runtime_config_raises_error_when_action_service_does_not_exist_in_services() {
        let config = r#"{
            "services": {
                "limitador": {
                    "type": "ratelimit",
                    "endpoint": "limitador",
                    "failureMode": "allow"
                }
            },
            "actionSets": [
            {
                "name": "some-name",
                "routeRuleConditions": {
                    "hostnames": ["*.example.com"]
                },
                "actions": [
                {
                    "service": "unknown",
                    "scope": "some-scope",
                    "data": [
                    {
                        "expression": {
                            "key": "a",
                            "value": "1"
                        }
                    }]
                }]
            }]
        }"#;
        let serde_res = serde_json::from_str::<PluginConfiguration>(config);
        if let Err(ref e) = serde_res {
            eprintln!("{e}");
        }
        assert!(serde_res.is_ok());

        let result = RuntimeConfig::try_from(serde_res.expect("That didn't work"));
        assert_eq!(result.err(), Some("Unknown service: unknown".into()));
    }
}
