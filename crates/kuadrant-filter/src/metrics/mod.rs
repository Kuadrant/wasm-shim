use std::collections::BTreeMap;
use std::sync::{LazyLock, OnceLock};

const CONFIGS: &str = "kuadrant.configs";
const HITS: &str = "kuadrant.hits";
const MISSES: &str = "kuadrant.misses";
const ALLOW: &str = "kuadrant.allowed";
const DENIED: &str = "kuadrant.denied";
const ERRORS: &str = "kuadrant.errors";

const NOOP: Counter = Counter(None);

pub trait MetricsBackend: Send + Sync {
    fn define_counter(&self, name: &str) -> Option<u32>;
    fn increment_counter(&self, id: u32, offset: i64);
}

static METRICS_BACKEND: OnceLock<Box<dyn MetricsBackend>> = OnceLock::new();

pub fn register_backend(backend: Box<dyn MetricsBackend>) {
    let _ = METRICS_BACKEND.set(backend);
}

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
            let id = METRICS_BACKEND.get().and_then(|b| b.define_counter(metric));
            counters.insert(metric.to_string(), Counter(id));
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
        if let (Some(id), Some(backend)) = (self.0, METRICS_BACKEND.get()) {
            backend.increment_counter(id, offset);
        }
    }
}
