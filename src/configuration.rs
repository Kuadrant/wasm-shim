use crate::envoy::RLA_action_specifier;
use regex::Regex;
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
pub enum Operation {
    Authenticate(Authentication),
    RateLimit(RateLimit),
}

impl Operation {
    pub fn is_excluded(&self, path: &str) -> bool {
        let exclude_pattern = match self {
            Operation::Authenticate(inner) => inner.exclude_pattern(),
            Operation::RateLimit(inner) => inner.exclude_pattern(),
        };

        if let Some(pattern) = exclude_pattern {
            return pattern.is_match(path);
        }
        false
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct Authentication {
    // Upstream Authorino's service name.
    upstream_cluster: String,
    // Regex pattern to exclude from authentication.
    #[serde(with = "serde_regex")]
    exclude_pattern: Option<Regex>,
}

impl Authentication {
    pub fn upstream_cluster(&self) -> &str {
        &self.upstream_cluster
    }

    pub fn exclude_pattern(&self) -> Option<&Regex> {
        match &self.exclude_pattern {
            Some(regex) => Some(regex),
            None => None,
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct RateLimit {
    // Upstream Limitador's service name.
    upstream_cluster: String,
    // RL domain to use when calling the ratelimit service.
    domain: String,
    // Envoy actions for generating descriptor keys.
    actions: Vec<RLA_action_specifier>,
    // Regex pattern to exclude from ratelimiting.
    #[serde(with = "serde_regex")]
    exclude_pattern: Option<Regex>,
}

impl RateLimit {
    pub fn upstream_cluster(&self) -> &str {
        &self.upstream_cluster
    }

    pub fn exclude_pattern(&self) -> Option<&Regex> {
        match &self.exclude_pattern {
            Some(regex) => Some(regex),
            None => None,
        }
    }

    pub fn actions(&self) -> &Vec<RLA_action_specifier> {
        &self.actions
    }

    pub fn domain(&self) -> &str {
        &self.domain
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct FilterConfig {
    // List of operations to apply on each request.
    operations: Vec<Operation>,
    // Deny request when faced with an irrecoverable failure.
    failure_mode_deny: bool,
}

impl FilterConfig {
    pub fn new() -> Self {
        Self {
            operations: Vec::new(),
            failure_mode_deny: true,
        }
    }

    pub fn mut_operations(&mut self) -> &mut Vec<Operation> {
        &mut self.operations
    }

    pub fn operations(&self) -> &Vec<Operation> {
        &self.operations
    }

    pub fn failure_mode_deny(&self) -> bool {
        self.failure_mode_deny
    }
}
