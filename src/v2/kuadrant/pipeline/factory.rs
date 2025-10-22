use crate::v2::configuration::{PluginConfiguration, Service};
use crate::v2::data::Expression;
use crate::v2::kuadrant::pipeline::blueprint::{Blueprint, CompileError};
use radix_trie::Trie;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

type RequestData = ((String, String), Expression);

pub struct PipelineFactory {
    index: Trie<String, Vec<Rc<Blueprint>>>,
    services: HashMap<String, Rc<Service>>,
    request_data: Arc<Vec<RequestData>>,
}

impl TryFrom<PluginConfiguration> for PipelineFactory {
    type Error = CompileError;

    fn try_from(config: PluginConfiguration) -> Result<Self, Self::Error> {
        let services: HashMap<String, Rc<Service>> = config
            .services
            .into_iter()
            .map(|(name, svc)| (name, Rc::new(svc)))
            .collect();

        let mut index = Trie::new();
        for config_action_set in &config.action_sets {
            let blueprint = Rc::new(Blueprint::compile(config_action_set, &services)?);

            for hostname in &config_action_set.route_rule_conditions.hostnames {
                let key = reverse_subdomain(hostname);
                index.map_with_default(
                    key,
                    |blueprints| blueprints.push(Rc::clone(&blueprint)),
                    vec![Rc::clone(&blueprint)],
                );
            }
        }

        let mut request_data: Vec<((String, String), Expression)> = config
            .request_data
            .iter()
            .filter_map(|(k, v)| {
                Expression::new(v).ok().map(|expr| {
                    let (domain, field) = domain_and_field_name(k);
                    ((domain.to_string(), field.to_string()), expr)
                })
            })
            .collect();
        request_data.sort_by(|a, b| a.0.cmp(&b.0));

        Ok(Self {
            index,
            services,
            request_data: Arc::new(request_data),
        })
    }
}

fn reverse_subdomain(subdomain: &str) -> String {
    let mut s = subdomain.to_string();
    s.push('.');
    if s.starts_with('*') {
        s.remove(0);
    } else {
        s.insert(0, '$');
    }
    s.chars().rev().collect()
}

fn domain_and_field_name(name: &str) -> (&str, &str) {
    let haystack = &name[..name
        .char_indices()
        .rfind(|(_, c)| c.is_alphabetic())
        .map(|(i, _)| i)
        .unwrap_or_default()];
    haystack
        .rfind('.')
        .map(|i| {
            if i == 0 || i == name.len() - 1 {
                ("", name)
            } else {
                (&name[..i], &name[i + 1..])
            }
        })
        .unwrap_or(("", name))
}
