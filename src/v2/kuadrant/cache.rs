use log::warn;
use radix_trie::Trie;
use std::collections::HashMap;
use std::sync::Mutex;

use crate::v2::data::attribute::{AttributeError, AttributeState, AttributeValue, Path};

#[derive(Clone, Debug, PartialEq)]
pub enum CachedValue {
    Bytes(Option<Vec<u8>>),
    Map(HashMap<String, String>),
}

pub struct AttributeCache {
    inner: Mutex<Trie<String, CachedValue>>,
}

impl AttributeCache {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Trie::new()),
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
    ) -> Result<AttributeState<Option<T>>, AttributeError>
    where
        F: FnOnce() -> Result<CachedValue, AttributeError>,
    {
        if let Ok(Some(cached)) = self.get(path) {
            return Ok(AttributeState::Available(T::from_cached(&cached)?));
        }

        match f() {
            Ok(cached_value) => {
                if let Err(e) = self.insert(path.clone(), cached_value.clone()) {
                    warn!("Failed to cache attribute {}: {}", path, e);
                }
                Ok(AttributeState::Available(T::from_cached(&cached_value)?))
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

    pub fn populate<F>(&self, path: &Path, f: F) -> Result<(), AttributeError>
    where
        F: FnOnce() -> Result<CachedValue, AttributeError>,
    {
        if self.get(path)?.is_some() {
            return Ok(());
        }

        match f() {
            Ok(cached_value) => self.insert(path.clone(), cached_value),
            Err(AttributeError::NotAvailable(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_insert_and_get_bytes() {
        let cache = AttributeCache::new();
        let path: Path = "test.bytes".into();
        let value = CachedValue::Bytes(Some(b"test data".to_vec()));

        cache.insert(path.clone(), value.clone()).unwrap();

        assert_eq!(cache.get(&path).unwrap(), Some(value));
        assert!(cache.contains_key(&path).unwrap());
    }

    #[test]
    fn test_insert_and_get_map() {
        let cache = AttributeCache::new();
        let path: Path = "test.map".into();
        let mut map = HashMap::new();
        map.insert("key1".to_string(), "value1".to_string());
        map.insert("key2".to_string(), "value2".to_string());
        let value = CachedValue::Map(map);

        cache.insert(path.clone(), value.clone()).unwrap();

        assert_eq!(cache.get(&path).unwrap(), Some(value));
        assert!(cache.contains_key(&path).unwrap());
    }

    #[test]
    fn test_get_or_insert_with_cache_miss() {
        let cache = AttributeCache::new();
        let path: Path = "test.miss".into();

        let result: Result<AttributeState<Option<String>>, _> = cache
            .get_or_insert_with(&path, || {
                Ok(CachedValue::Bytes(Some(b"new value".to_vec())))
            });

        assert!(matches!(result, Ok(AttributeState::Available(Some(ref s))) if s == "new value"));
        assert!(cache.contains_key(&path).unwrap());
    }

    #[test]
    fn test_get_or_insert_with_cache_hit() {
        let cache = AttributeCache::new();
        let path: Path = "test.hit".into();
        let cached_value = CachedValue::Bytes(Some(b"cached".to_vec()));

        cache.insert(path.clone(), cached_value).unwrap();

        let result: Result<AttributeState<Option<String>>, _> = cache
            .get_or_insert_with(&path, || {
                Ok(CachedValue::Bytes(Some(b"should not be called".to_vec())))
            });

        assert!(matches!(result, Ok(AttributeState::Available(Some(ref s))) if s == "cached"));
    }

    #[test]
    fn test_get_or_insert_with_not_available() {
        let cache = AttributeCache::new();
        let path: Path = "test.unavailable".into();

        let result: Result<AttributeState<Option<String>>, _> = cache
            .get_or_insert_with(&path, || {
                Err(AttributeError::NotAvailable("not ready".to_string()))
            });

        assert!(matches!(result, Ok(AttributeState::Pending)));
        assert!(!cache.contains_key(&path).unwrap());
    }

    #[test]
    fn test_populate_cache_miss() {
        let cache = AttributeCache::new();
        let path: Path = "test.populate.miss".into();

        let result = cache.populate(&path, || {
            Ok(CachedValue::Bytes(Some(b"populated".to_vec())))
        });

        assert!(result.is_ok());
        assert!(cache.contains_key(&path).unwrap());
        assert_eq!(
            cache.get(&path).unwrap(),
            Some(CachedValue::Bytes(Some(b"populated".to_vec())))
        );
    }

    #[test]
    fn test_populate_cache_hit() {
        let cache = AttributeCache::new();
        let path: Path = "test.populate.hit".into();
        let original_value = CachedValue::Bytes(Some(b"original".to_vec()));

        cache.insert(path.clone(), original_value.clone()).unwrap();

        let result = cache.populate(&path, || {
            Ok(CachedValue::Bytes(Some(b"should not be inserted".to_vec())))
        });

        assert!(result.is_ok());
        assert_eq!(cache.get(&path).unwrap(), Some(original_value));
    }

    #[test]
    fn test_populate_not_available() {
        let cache = AttributeCache::new();
        let path: Path = "test.populate.unavailable".into();

        let result = cache.populate(&path, || {
            Err(AttributeError::NotAvailable("not ready".to_string()))
        });

        assert!(result.is_ok());
        assert!(!cache.contains_key(&path).unwrap());
    }

    #[test]
    fn test_populate_error() {
        let cache = AttributeCache::new();
        let path: Path = "test.populate.error".into();

        let result = cache.populate(&path, || {
            Err(AttributeError::Retrieval("some error".to_string()))
        });

        assert!(matches!(result, Err(AttributeError::Retrieval(_))));
        assert!(!cache.contains_key(&path).unwrap());
    }
}
