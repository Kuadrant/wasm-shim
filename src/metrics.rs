use log::error;
use proxy_wasm::hostcalls;
use std::collections::HashMap;
use std::string::ToString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, RwLock};

pub trait Metrics {
    fn increment_authorized_calls(&self, _scope: &str) {}
    fn increment_limited_calls(&self, _scope: &str) {}

    fn increment_token_usage(&self, _scope: &str, _tokens: i64) {}

    fn is_enabled(&self) -> bool {
        false
    }
}

struct NoopMetrics {}

impl Metrics for NoopMetrics {}

struct HostMetrics {
    enabled: AtomicBool,
    metrics: RwLock<HashMap<String, u32>>,
}

impl HostMetrics {
    const AUTHORIZED_CALLS_METRIC_ID: &'static str = "authorized_calls_total";
    const LIMITED_CALLS_METRIC_ID: &'static str = "limited_calls_total";
    const TOKEN_USAGE_METRIC_ID: &'static str = "token_usage_total";

    pub fn new() -> Self {
        let mut metrics = HashMap::new();

        for name in [
            Self::AUTHORIZED_CALLS_METRIC_ID,
            Self::LIMITED_CALLS_METRIC_ID,
            Self::TOKEN_USAGE_METRIC_ID,
        ] {
            match hostcalls::define_metric(proxy_wasm::types::MetricType::Counter, name) {
                Ok(metric_id) => {
                    metrics.insert(name.to_string(), metric_id);
                }
                Err(e) => error!("Failed to define {name} metric: {e:?}"),
            }
        }

        Self {
            enabled: AtomicBool::new(true),
            metrics: RwLock::new(metrics),
        }
    }

    #[allow(dead_code)]
    pub fn enable(&self) {
        self.enabled.store(true, Ordering::Release);
    }

    #[allow(dead_code)]
    pub fn disable(&self) {
        self.enabled.store(true, Ordering::Release);
    }

    fn try_offset_counter(&self, name: &str, offset: i64) {
        if self.is_enabled() {
            if let Ok(metrics) = self.metrics.read() {
                if let Some(metric) = metrics.get(name) {
                    if let Err(e) = hostcalls::increment_metric(*metric, offset) {
                        error!("Failed to increment `{name}` metric: {e:?}");
                    }
                }
            }
        }
    }

    fn increment_authorized_calls_with_user_group(&self, scope: &str) {
        let (user_id, group) = extract_user_info();

        if user_id == "unknown" || group == "unknown" {
            return;
        }

        let namespace = extract_namespace_from_scope(scope);

        if let Some(metric_id) = self.get_or_create_user_group_metric(
            "authorized_calls_with_user_and_group",
            &user_id,
            &group,
            &namespace,
        ) {
            match hostcalls::increment_metric(metric_id, 1) {
                Ok(_) => {}
                Err(e) => error!(
                    "Failed to increment authorized_calls_with_user_and_group: {:?}",
                    e
                ),
            }
        }
    }

    fn increment_limited_calls_with_user_group(&self, scope: &str) {
        let (user_id, group) = extract_user_info();

        if user_id == "unknown" || group == "unknown" {
            return;
        }

        let namespace = extract_namespace_from_scope(scope);

        if let Some(metric_id) = self.get_or_create_user_group_metric(
            "limited_calls_with_user_and_group",
            &user_id,
            &group,
            &namespace,
        ) {
            match hostcalls::increment_metric(metric_id, 1) {
                Ok(_) => {}
                Err(e) => error!(
                    "Failed to increment limited_calls_with_user_and_group: {:?}",
                    e
                ),
            }
        }
    }

    fn increment_token_usage_with_user_group(&self, scope: &str, tokens: i64) {
        let (user_id, group) = extract_user_info();

        if user_id == "unknown" || group == "unknown" {
            return;
        }

        let namespace = extract_namespace_from_scope(scope);

        if let Some(metric_id) = self.get_or_create_user_group_metric(
            "token_usage_with_user_and_group",
            &user_id,
            &group,
            &namespace,
        ) {
            match hostcalls::increment_metric(metric_id, tokens) {
                Ok(_) => {}
                Err(e) => error!(
                    "Failed to increment token_usage_with_user_and_group: {:?}",
                    e
                ),
            }
        }
    }

    fn get_or_create_user_group_metric(
        &self,
        metric_type: &str,
        user: &str,
        group: &str,
        namespace: &str,
    ) -> Option<u32> {
        // Format: metric_type__user__USER__group__GROUP__namespace__NAMESPACE
        let metric_name =
            format!("{metric_type}__user__{user}__group__{group}__namespace__{namespace}");

        let map_key = format!("{metric_type}:{user}:{group}:{namespace}");

        if let Ok(ref mut metrics_map) = self.metrics.write() {
            if let Some(&metric_id) = metrics_map.get(&map_key) {
                return Some(metric_id);
            }

            // Create new metric
            match hostcalls::define_metric(proxy_wasm::types::MetricType::Counter, &metric_name) {
                Ok(metric_id) => {
                    metrics_map.insert(map_key, metric_id);
                    Some(metric_id)
                }
                Err(e) => {
                    error!(
                        "Failed to define user/group metric {}: {:?}",
                        metric_name, e
                    );
                    None
                }
            }
        } else {
            error!("User/group metrics poisoned!");
            None
        }
    }
}

impl Metrics for HostMetrics {
    fn increment_authorized_calls(&self, scope: &str) {
        self.try_offset_counter(Self::AUTHORIZED_CALLS_METRIC_ID, 1);
        self.increment_authorized_calls_with_user_group(scope);
    }
    fn increment_limited_calls(&self, scope: &str) {
        self.try_offset_counter(Self::LIMITED_CALLS_METRIC_ID, 1);
        self.increment_limited_calls_with_user_group(scope);
    }

    fn increment_token_usage(&self, scope: &str, tokens: i64) {
        self.try_offset_counter(Self::TOKEN_USAGE_METRIC_ID, tokens);
        self.increment_token_usage_with_user_group(scope, tokens);
    }

    fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }
}

static METRICS: OnceLock<Box<dyn Metrics + Sync + Send>> = OnceLock::new();

pub fn get_metrics() -> &'static dyn Metrics {
    METRICS.get_or_init(|| Box::new(NoopMetrics {})).as_ref()
}

// Helper function to extract token count from response body JSON
fn extract_token_count_from_response_body() -> Option<i64> {
    // Try to get the response body buffer
    match proxy_wasm::hostcalls::get_buffer(
        proxy_wasm::types::BufferType::HttpResponseBody,
        0,
        usize::MAX,
    ) {
        Ok(Some(body_bytes)) => {
            match String::from_utf8(body_bytes) {
                Ok(body_str) => {
                    // Try to parse as JSON
                    match serde_json::from_str::<serde_json::Value>(&body_str) {
                        Ok(json) => {
                            // Extract total token usage from `usage.total_tokens`
                            let token_paths: &[&[&str]] = &[&["usage", "total_tokens"]];

                            for path in token_paths {
                                if let Some(tokens) = extract_json_path(&json, path) {
                                    return Some(tokens);
                                }
                            }
                        }
                        Err(_e) => {}
                    }
                }
                Err(_e) => {}
            }
        }
        Ok(None) => {}
        Err(_e) => {}
    }

    None
}

// Helper function to extract value from nested JSON path
fn extract_json_path(json: &serde_json::Value, path: &[&str]) -> Option<i64> {
    let mut current = json;

    for segment in path {
        match current.get(segment) {
            Some(value) => current = value,
            None => return None,
        }
    }

    // Try to convert to i64
    match current {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => s.parse::<i64>().ok(),
        _ => None,
    }
}

// Helper function to extract user info from auth metadata
fn extract_user_info() -> (String, String) {
    let user_id_path = crate::data::wasm_prop(&["auth", "identity", "userid"]);
    let user_id = match crate::data::get_attribute::<String>(&user_id_path) {
        Ok(Some(id)) => id,
        _ => "unknown".to_string(),
    };

    let user_groups_path = crate::data::wasm_prop(&["auth", "identity", "groups"]);
    let group = match crate::data::get_attribute::<String>(&user_groups_path) {
        Ok(Some(groups)) => groups.split(',').next().unwrap_or("unknown").to_string(),
        _ => "unknown".to_string(),
    };

    (user_id, group)
}

fn extract_namespace_from_scope(scope: &str) -> String {
    if scope.contains('/') {
        scope.split('/').next().unwrap_or("default").to_string()
    } else {
        "default".to_string()
    }
}

pub fn process_response_body_for_token_usage(scope: &str) {
    let metrics = get_metrics();
    if metrics.is_enabled() {
        if let Some(token_count) = extract_token_count_from_response_body() {
            metrics.increment_token_usage(scope, token_count);
        }
    }
}

#[allow(dead_code)]
pub fn initialize_metrics() {
    METRICS.get_or_init(|| Box::new(HostMetrics::new()));
}
