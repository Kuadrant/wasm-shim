use log::warn;
use radix_trie::Trie;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::v2::data::attribute::{AttributeError, AttributeState, AttributeValue, Path};

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

    pub fn get_or_insert_with<T: AttributeValue, F>(
        &self,
        path: &Path,
        f: F,
    ) -> Result<AttributeState<T>, AttributeError>
    where
        F: FnOnce() -> Result<CachedValue, AttributeError>,
    {
        if let Ok(Some(cached)) = self.get(path) {
            return match T::from_cached(&cached)? {
                Some(value) => Ok(AttributeState::Available(Some(value))),
                None => Ok(AttributeState::Available(None)),
            };
        }

        match f() {
            Ok(cached_value) => {
                if let Err(e) = self.insert(path.clone(), cached_value.clone()) {
                    warn!("Failed to cache attribute {}: {}", path, e);
                }

                match T::from_cached(&cached_value)? {
                    Some(value) => Ok(AttributeState::Available(Some(value))),
                    None => Ok(AttributeState::Available(None)),
                }
            }
            Err(AttributeError::NotAvailable(_)) => Ok(AttributeState::Pending),
            Err(e) => Err(e),
        }
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
