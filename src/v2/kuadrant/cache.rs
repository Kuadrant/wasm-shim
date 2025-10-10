use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::v2::data::attribute::Path;

#[derive(Clone)]
pub struct AttributeCache {
    inner: Arc<Mutex<HashMap<Path, Option<Vec<u8>>>>>,
}

impl AttributeCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn get(&self, path: &Path) -> Option<Option<Vec<u8>>> {
        self.inner
            .lock()
            .expect("cache mutex not poisoned")
            .get(path)
            .cloned()
    }

    pub fn insert(&self, path: Path, value: Option<Vec<u8>>) {
        self.inner
            .lock()
            .expect("cache mutex not poisoned")
            .insert(path, value);
    }

    pub fn contains_key(&self, path: &Path) -> bool {
        self.inner
            .lock()
            .expect("cache mutex not poisoned")
            .contains_key(path)
    }
}
