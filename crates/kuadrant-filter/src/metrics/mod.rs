use std::collections::BTreeMap;
use std::sync::LazyLock;

const CONFIGS: &str = "kuadrant.configs";
const HITS: &str = "kuadrant.hits";
const MISSES: &str = "kuadrant.misses";
const ALLOW: &str = "kuadrant.allowed";
const DENIED: &str = "kuadrant.denied";
const ERRORS: &str = "kuadrant.errors";

const NOOP: Counter = Counter(None);

pub struct Metrics {
    counters: BTreeMap<String, Counter>,
}

pub static METRICS: LazyLock<Metrics> = LazyLock::new(Metrics::default);

impl Metrics {
    fn get_counter(&self, name: &str) -> &Counter {
        self.counters.get(name).unwrap_or(&NOOP)
    }

    pub fn configs(&self) -> &Counter {
        self.get_counter(CONFIGS)
    }

    pub fn hits(&self) -> &Counter {
        self.get_counter(HITS)
    }

    pub fn misses(&self) -> &Counter {
        self.get_counter(MISSES)
    }

    pub fn allowed(&self) -> &Counter {
        self.get_counter(ALLOW)
    }

    pub fn denied(&self) -> &Counter {
        self.get_counter(DENIED)
    }

    pub fn errors(&self) -> &Counter {
        self.get_counter(ERRORS)
    }
}

impl Default for Metrics {
    fn default() -> Self {
        let mut counters = BTreeMap::new();

        for metric in [CONFIGS, HITS, MISSES, ALLOW, DENIED, ERRORS] {
            let result = if cfg!(target_arch = "wasm32") {
                proxy_wasm::hostcalls::define_metric(proxy_wasm::types::MetricType::Counter, metric)
            } else {
                Ok(0)
            };
            match result {
                Ok(id) => {
                    counters.insert(metric.to_string(), Counter(Some(id)));
                }
                Err(_) => tracing::error!("failed to add metric: {}", metric),
            }
        }

        Self { counters }
    }
}

pub struct Counter(Option<u32>);

impl Counter {
    pub fn increment(&self) {
        self.inc_by(1);
    }

    pub fn inc_by(&self, offset: i64) {
        if cfg!(target_arch = "wasm32") {
            if let Some(id) = self.0 {
                let _ = proxy_wasm::hostcalls::increment_metric(id, offset);
            }
        }
    }
}
