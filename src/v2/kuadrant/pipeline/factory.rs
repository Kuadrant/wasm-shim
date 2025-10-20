use crate::v2::configuration::{PluginConfiguration, Service};
use crate::v2::data::{attribute::AttributeState, Expression};
use crate::v2::kuadrant::pipeline::blueprint::{Blueprint, CompileError};
use crate::v2::kuadrant::pipeline::executor::Pipeline;
use crate::v2::kuadrant::ReqRespCtx;
use cel_interpreter::Value;
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

#[derive(Debug)]
pub enum BuildError {
    DataPending(String),
    EvaluationError(String),
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

impl Default for PipelineFactory {
    fn default() -> Self {
        Self {
            index: Trie::new(),
            services: HashMap::new(),
            request_data: Arc::new(Vec::new()),
        }
    }
}

impl PipelineFactory {
    pub fn build(&self, ctx: ReqRespCtx) -> Result<Option<Pipeline>, BuildError> {
        let ctx = ctx.with_request_data(Arc::clone(&self.request_data));

        let _blueprint = match self.select_blueprint(&ctx)? {
            Some(bp) => bp,
            None => return Ok(None),
        };

        // TODO: Create tasks from blueprint.actions
        Ok(Some(Pipeline::new(ctx)))
    }

    fn select_blueprint(&self, ctx: &ReqRespCtx) -> Result<Option<&Rc<Blueprint>>, BuildError> {
        let hostname = self.get_hostname(ctx)?;

        let candidates = match self.index.get_ancestor_value(&reverse_subdomain(&hostname)) {
            Some(blueprints) => blueprints,
            None => return Ok(None),
        };

        for blueprint in candidates {
            if self.route_predicates_match(&blueprint.route_predicates, &blueprint.name, ctx)? {
                return Ok(Some(blueprint));
            }
        }

        Ok(None)
    }

    fn get_hostname(&self, ctx: &ReqRespCtx) -> Result<String, BuildError> {
        match ctx.get_attribute::<String>("request.host") {
            Ok(AttributeState::Available(Some(hostname))) => Ok(hostname),
            Ok(AttributeState::Available(None)) => Err(BuildError::EvaluationError(
                "hostname not found".to_string(),
            )),
            Ok(AttributeState::Pending) => Err(BuildError::DataPending("hostname".to_string())),
            Err(e) => Err(BuildError::EvaluationError(e.to_string())),
        }
    }

    fn route_predicates_match(
        &self,
        predicates: &[Expression],
        blueprint_name: &str,
        ctx: &ReqRespCtx,
    ) -> Result<bool, BuildError> {
        if predicates.is_empty() {
            return Ok(true);
        }

        for predicate in predicates {
            match predicate.eval(ctx) {
                Ok(AttributeState::Available(Value::Bool(true))) => continue,
                Ok(AttributeState::Available(Value::Bool(false))) => return Ok(false),
                Ok(AttributeState::Available(value)) => {
                    return Err(BuildError::EvaluationError(format!(
                        "route predicate returned non-boolean: {:?}",
                        value
                    )))
                }
                Ok(AttributeState::Pending) => {
                    return Err(BuildError::DataPending(format!(
                        "route predicate: {}",
                        blueprint_name
                    )))
                }
                Err(e) => {
                    return Err(BuildError::EvaluationError(format!(
                        "route predicate evaluation failed: {}",
                        e
                    )))
                }
            }
        }

        Ok(true)
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
