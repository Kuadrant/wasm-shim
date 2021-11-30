#[derive(Deserialize, Debug, Clone)]
struct FilterConfig {
    // Upstream Authorino's service name.
    auth_cluster: String,
    // Deny request when faced with an irrecoverable failure.
    failure_mode_deny: bool,
}

impl FilterConfig {
    pub fn new() -> Self {
        Self {
            auth_cluster: String::new(),
            failure_mode_deny: true,
        }
    }
    fn auth_cluster(&self) -> &str {
        self.auth_cluster.as_ref()
    }

    fn failure_mode_deny(&self) -> bool {
        self.failure_mode_deny
    }
}