use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::v2::data::attribute::{AttributeError, Path};

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

    pub fn get(&self, path: &Path) -> Result<Option<Option<Vec<u8>>>, AttributeError> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| AttributeError::Retrieval("cache mutex poisoned".to_string()))?;
        Ok(guard.get(path).cloned())
    }

    pub fn insert(&self, path: Path, value: Option<Vec<u8>>) -> Result<(), AttributeError> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| AttributeError::Retrieval("cache mutex poisoned".to_string()))?;
        guard.insert(path, value);
        Ok(())
    }

    pub fn contains_key(&self, path: &Path) -> Result<bool, AttributeError> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| AttributeError::Retrieval("cache mutex poisoned".to_string()))?;
        Ok(guard.contains_key(path))
    }
}
