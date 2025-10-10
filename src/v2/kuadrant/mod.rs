use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::v2::data::attribute::{AttributeError, AttributeState, AttributeValue, Path};
use crate::v2::resolver::AttributeResolver;
use log::warn;

#[derive(Clone)]
struct AttributeCache {
    inner: Arc<Mutex<HashMap<Path, Option<Vec<u8>>>>>,
}

impl AttributeCache {
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn get(&self, path: &Path) -> Option<Option<Vec<u8>>> {
        self.inner
            .lock()
            .expect("cache mutex not poisoned")
            .get(path)
            .cloned()
    }

    fn insert(&self, path: Path, value: Option<Vec<u8>>) {
        self.inner
            .lock()
            .expect("cache mutex not poisoned")
            .insert(path, value);
    }

    fn contains_key(&self, path: &Path) -> bool {
        self.inner
            .lock()
            .expect("cache mutex not poisoned")
            .contains_key(path)
    }
}

#[derive(Clone)]
pub struct ReqRespCtx {
    backend: Arc<dyn AttributeResolver>,
    cache: AttributeCache,
}

impl ReqRespCtx {
    pub fn new(backend: Arc<dyn AttributeResolver + 'static>) -> Self {
        Self {
            backend,
            cache: AttributeCache::new(),
        }
    }

    pub fn get_attribute<T: AttributeValue>(
        &self,
        path: impl Into<Path>,
    ) -> Result<AttributeState<T>, AttributeError> {
        self.get_attribute_ref(&path.into())
    }

    pub fn get_attribute_ref<T: AttributeValue>(
        &self,
        path: &Path,
    ) -> Result<AttributeState<T>, AttributeError> {
        if let Some(cached_option) = self.cache.get(path) {
            return match cached_option {
                Some(bytes) => match T::parse(bytes.clone()) {
                    Ok(parsed) => Ok(AttributeState::Available(Some(parsed))),
                    Err(parse_err) => Err(parse_err),
                },
                None => Ok(AttributeState::Available(None)),
            };
        }

        let raw_result = self.fetch_attribute(path);
        match raw_result {
            Ok(option_bytes) => {
                self.cache.insert(path.clone(), option_bytes.clone());
                match option_bytes {
                    Some(bytes) => match T::parse(bytes) {
                        Ok(parsed) => Ok(AttributeState::Available(Some(parsed))),
                        Err(parse_err) => Err(parse_err),
                    },
                    None => Ok(AttributeState::Available(None)),
                }
            }
            Err(AttributeError::NotAvailable(_)) => Ok(AttributeState::Pending),
            Err(e) => Err(e),
        }
    }

    pub fn get_attribute_map(
        &self,
        path: &Path,
    ) -> Result<HashMap<String, String>, AttributeError> {
        match *path.tokens() {
            ["request", "headers"] => {
                match self
                    .backend
                    .get_attribute_map(proxy_wasm::types::MapType::HttpRequestHeaders)
                {
                    Ok(map) => Ok(map),
                    Err(err) => Err(err),
                }
            }
            _ => Err(AttributeError::Retrieval(format!(
                "Unknown map requested: {}",
                path
            ))),
        }
    }

    pub fn ensure_attributes(&self, paths: &[Path]) {
        for path in paths {
            if !self.cache.contains_key(path) {
                if let Ok(option_bytes) = self.fetch_attribute(path) {
                    self.cache.insert(path.clone(), option_bytes);
                }
            }
        }
    }

    fn fetch_attribute(&self, path: &Path) -> Result<Option<Vec<u8>>, AttributeError> {
        match *path.tokens() {
            ["source", "remote_address"] => self.remote_address(),
            ["auth", ..] => self.backend.get_attribute(&wasm_prop(&path.tokens())),
            _ => self.backend.get_attribute(path),
        }
    }

    fn remote_address(&self) -> Result<Option<Vec<u8>>, AttributeError> {
        // Ref https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_conn_man/headers#x-forwarded-for
        // Envoy sets source.address to the trusted client address AND port.
        match self.backend.get_attribute(&"source.address".into()) {
            Ok(Some(host_vec)) => match String::from_utf8(host_vec) {
                Ok(source_address) => {
                    let split_address = source_address.split(':').collect::<Vec<_>>();
                    Ok(Some(split_address[0].as_bytes().to_vec()))
                }
                Err(_) => Err(AttributeError::Parse(
                    "source.address not valid UTF-8".to_string(),
                )),
            },
            Ok(None) => {
                warn!("source.address property not found");
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }
}

pub fn wasm_prop(tokens: &[&str]) -> Path {
    let mut flat_attr = "filter_state.wasm\\.kuadrant\\.".to_string();
    flat_attr.push_str(tokens.join("\\.").as_str());
    flat_attr.as_str().into()
}

#[cfg(test)]
pub mod tests {
    use crate::v2::{
        data::attribute::AttributeState, kuadrant::ReqRespCtx, resolver::MockWasmHost,
    };
    use std::sync::Arc;

    #[cfg(test)]
    mod cache_tests {
        use super::*;

        #[test]
        fn test_caching_basic_functionality() {
            let mock_host =
                MockWasmHost::new().with_property("request.method".into(), "GET".bytes().collect());
            let ctx = ReqRespCtx::new(Arc::new(mock_host));

            let result1: Result<AttributeState<String>, _> = ctx.get_attribute("request.method");
            assert!(result1.is_ok());
            if let Ok(AttributeState::Available(Some(method))) = result1 {
                assert_eq!(method, "GET");
            } else {
                panic!("Expected Available(Some(GET))");
            }

            // check it is cached
            assert!(ctx.cache.contains_key(&"request.method".into()));

            // second access uses cache
            let result2: Result<AttributeState<String>, _> = ctx.get_attribute("request.method");
            assert!(result2.is_ok());
            if let Ok(AttributeState::Available(Some(method))) = result2 {
                assert_eq!(method, "GET");
            } else {
                panic!("Expected Available(Some(GET)) from cache");
            }
        }

        #[test]
        fn test_ensure_attributes_batch_loading() {
            let mock_host = MockWasmHost::new()
                .with_property("request.method".into(), "POST".bytes().collect())
                .with_property("request.path".into(), "/api/test".bytes().collect());
            let ctx = ReqRespCtx::new(Arc::new(mock_host));

            let paths = vec!["request.method".into(), "request.path".into()];
            ctx.ensure_attributes(&paths);

            // both are cached
            assert!(ctx.cache.contains_key(&"request.method".into()));
            assert!(ctx.cache.contains_key(&"request.path".into()));

            // accessing uses cache
            let method: Result<AttributeState<String>, _> = ctx.get_attribute("request.method");
            let path: Result<AttributeState<String>, _> = ctx.get_attribute("request.path");

            assert!(method.is_ok());
            assert!(path.is_ok());
        }
    }
}
