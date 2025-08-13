use log::error;
use proxy_wasm::hostcalls;
use std::collections::HashMap;

// Simple metric IDs
static mut AUTHORIZED_CALLS_METRIC_ID: Option<u32> = None;
static mut LIMITED_CALLS_METRIC_ID: Option<u32> = None;
static mut TOKEN_USAGE_METRIC_ID: Option<u32> = None;

// Storage for user/group specific metrics
static mut USER_GROUP_METRICS: Option<HashMap<String, u32>> = None;

pub fn initialize_metrics() {
    // Initialize user/group metrics storage
    unsafe {
        USER_GROUP_METRICS = Some(HashMap::new());
    }

    // Authorized calls counter
    match hostcalls::define_metric(
        proxy_wasm::types::MetricType::Counter,
        "authorized_calls_total",
    ) {
        Ok(metric_id) => unsafe {
            AUTHORIZED_CALLS_METRIC_ID = Some(metric_id);
        },
        Err(e) => {
            error!("Failed to define authorized_calls_total metric: {:?}", e);
        }
    }

    // Limited calls counter
    match hostcalls::define_metric(
        proxy_wasm::types::MetricType::Counter,
        "limited_calls_total",
    ) {
        Ok(metric_id) => unsafe {
            LIMITED_CALLS_METRIC_ID = Some(metric_id);
        },
        Err(e) => {
            error!("Failed to define limited_calls_total metric: {:?}", e);
        }
    }

    // Token usage counter
    match hostcalls::define_metric(proxy_wasm::types::MetricType::Counter, "token_usage_total") {
        Ok(metric_id) => unsafe {
            TOKEN_USAGE_METRIC_ID = Some(metric_id);
        },
        Err(e) => {
            error!("Failed to define token_usage_total metric: {:?}", e);
        }
    }
}

pub fn increment_authorized_calls() {
    unsafe {
        if let Some(metric_id) = AUTHORIZED_CALLS_METRIC_ID {
            match hostcalls::increment_metric(metric_id, 1) {
                Ok(_) => {}
                Err(e) => error!("Failed to increment authorized_calls_total metric: {:?}", e),
            }
        } else {
            error!("Authorized calls metric not initialized");
        }
    }
}

pub fn increment_limited_calls() {
    unsafe {
        if let Some(metric_id) = LIMITED_CALLS_METRIC_ID {
            match hostcalls::increment_metric(metric_id, 1) {
                Ok(_) => {}
                Err(e) => error!("Failed to increment limited_calls_total metric: {:?}", e),
            }
        } else {
            error!("Limited calls metric not initialized");
        }
    }
}

pub fn increment_token_usage(tokens: i64) {
    unsafe {
        if let Some(metric_id) = TOKEN_USAGE_METRIC_ID {
            match hostcalls::increment_metric(metric_id, tokens) {
                Ok(_) => {}
                Err(e) => error!("Failed to increment token_usage_total metric: {:?}", e),
            }
        } else {
            error!("Token usage metric not initialized");
        }
    }
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

// Helper function to get or create a user/group specific metric with cleaner names
fn get_or_create_user_group_metric(
    metric_type: &str,
    user: &str,
    group: &str,
    namespace: &str,
) -> Option<u32> {
    // Format: metric_type__user__USER__group__GROUP__namespace__NAMESPACE
    let metric_name =
        format!("{metric_type}__user__{user}__group__{group}__namespace__{namespace}");

    let map_key = format!("{metric_type}:{user}:{group}:{namespace}");

    unsafe {
        if let Some(ref mut metrics_map) = USER_GROUP_METRICS {
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
            error!("User/group metrics not initialized");
            None
        }
    }
}

fn extract_namespace_from_scope(scope: &str) -> String {
    if scope.contains('/') {
        scope.split('/').next().unwrap_or("default").to_string()
    } else {
        "default".to_string()
    }
}

pub fn increment_authorized_calls_with_user_group(scope: &str) {
    let (user_id, group) = extract_user_info();

    if user_id == "unknown" || group == "unknown" {
        return;
    }

    let namespace = extract_namespace_from_scope(scope);

    if let Some(metric_id) = get_or_create_user_group_metric(
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

pub fn increment_limited_calls_with_user_group(scope: &str) {
    let (user_id, group) = extract_user_info();

    if user_id == "unknown" || group == "unknown" {
        return;
    }

    let namespace = extract_namespace_from_scope(scope);

    if let Some(metric_id) = get_or_create_user_group_metric(
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

pub fn increment_token_usage_with_user_group(tokens: i64, scope: &str) {
    let (user_id, group) = extract_user_info();

    if user_id == "unknown" || group == "unknown" {
        return;
    }

    let namespace = extract_namespace_from_scope(scope);

    if let Some(metric_id) = get_or_create_user_group_metric(
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

pub fn process_response_body_for_token_usage(scope: &str) {
    if let Some(token_count) = extract_token_count_from_response_body() {
        increment_token_usage(token_count);
        increment_token_usage_with_user_group(token_count, scope);
    }
}
