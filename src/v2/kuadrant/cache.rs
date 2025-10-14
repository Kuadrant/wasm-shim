use radix_trie::Trie;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::v2::data::attribute::{AttributeError, Path};

#[derive(Clone, Debug)]
pub enum CachedValue {
    Bytes(Option<Vec<u8>>),
    Map(HashMap<String, String>),
}

#[derive(Clone)]
pub struct AttributeCache {
    inner: Arc<Mutex<Trie<String, CachedValue>>>,
}

impl AttributeCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Trie::new())),
        }
    }

    pub fn get(&self, path: &Path) -> Result<Option<CachedValue>, AttributeError> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| AttributeError::Retrieval("cache mutex poisoned".to_string()))?;
        Ok(guard.get(&path.to_string()).cloned())
    }

    pub fn insert(&self, path: Path, value: CachedValue) -> Result<(), AttributeError> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| AttributeError::Retrieval("cache mutex poisoned".to_string()))?;
        guard.insert(path.to_string(), value);
        Ok(())
    }

    pub fn contains_key(&self, path: &Path) -> Result<bool, AttributeError> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| AttributeError::Retrieval("cache mutex poisoned".to_string()))?;
        Ok(guard.get(&path.to_string()).is_some())
    }
}
