use std::sync::Arc;

use crate::v2::data::attribute::{wasm_prop, AttributeError, AttributeState, AttributeValue, Path};
use crate::v2::data::cel::EvalResult;
use crate::v2::data::Expression;
use crate::v2::kuadrant::cache::CachedValue;
use crate::v2::kuadrant::resolver::AttributeResolver;
use crate::v2::kuadrant::AttributeCache;
use log::warn;

type RequestData = ((String, String), Expression);

#[derive(Clone)]
pub struct ReqRespCtx {
    backend: Arc<dyn AttributeResolver>,
    cache: Arc<AttributeCache>,
    request_data: Option<Arc<Vec<RequestData>>>,
}

impl ReqRespCtx {
    pub fn new(backend: Arc<dyn AttributeResolver + 'static>) -> Self {
        Self {
            backend,
            cache: Arc::new(AttributeCache::new()),
            request_data: None,
        }
    }

    pub fn with_request_data(mut self, request_data: &[RequestData]) -> Self {
        self.request_data = Some(Arc::new(request_data.to_vec()));
        self
    }

    pub fn get_attribute<T: AttributeValue>(
        &self,
        path: impl Into<Path>,
    ) -> Result<AttributeState<Option<T>>, AttributeError> {
        self.get_attribute_ref(&path.into())
    }

    pub fn get_attribute_ref<T: AttributeValue>(
        &self,
        path: &Path,
    ) -> Result<AttributeState<Option<T>>, AttributeError> {
        self.cache
            .get_or_insert_with(path, || self.fetch_attribute(path))
    }

    fn fetch_attribute(&self, path: &Path) -> Result<CachedValue, AttributeError> {
        match *path.tokens() {
            ["request", "headers"] => {
                let map = self
                    .backend
                    .get_attribute_map(proxy_wasm::types::MapType::HttpRequestHeaders)?;
                Ok(CachedValue::Map(map))
            }
            ["source", "remote_address"] => {
                let bytes = self.remote_address()?;
                Ok(CachedValue::Bytes(bytes))
            }
            ["auth", ..] => {
                let bytes = self.backend.get_attribute(&wasm_prop(&path.tokens()))?;
                Ok(CachedValue::Bytes(bytes))
            }
            _ => {
                let bytes = self.backend.get_attribute(path)?;
                Ok(CachedValue::Bytes(bytes))
            }
        }
    }

    pub fn ensure_attributes(&self, paths: &[Path]) {
        for path in paths {
            if let Err(e) = self.cache.populate(path, || self.fetch_attribute(path)) {
                warn!("Failed to ensure attribute {}: {}", path, e);
            }
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

    pub fn eval_request_data(&self) -> Vec<((String, String), EvalResult)> {
        let Some(ref expressions) = self.request_data else {
            return Vec::new();
        };
        expressions
            .iter()
            .map(|((domain, field), expr)| {
                let result = expr.eval(self);
                ((domain.clone(), field.clone()), result)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::data::cel::Expression;
    use crate::v2::{data::attribute::AttributeState, kuadrant::resolver::MockWasmHost};
    use std::sync::Arc;

    #[test]
    fn test_caching_basic_functionality() {
        let mock_host =
            MockWasmHost::new().with_property("request.method".into(), "GET".bytes().collect());
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let result1: Result<AttributeState<Option<String>>, _> =
            ctx.get_attribute("request.method");
        assert!(
            matches!(result1, Ok(AttributeState::Available(Some(ref method))) if method == "GET")
        );

        // check it is cached
        assert!(ctx
            .cache
            .contains_key(&"request.method".into())
            .unwrap_or(false));

        // second access uses cache
        let result2: Result<AttributeState<Option<String>>, _> =
            ctx.get_attribute("request.method");
        assert!(
            matches!(result2, Ok(AttributeState::Available(Some(ref method))) if method == "GET")
        );
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
        assert!(ctx
            .cache
            .contains_key(&"request.method".into())
            .unwrap_or(false));
        assert!(ctx
            .cache
            .contains_key(&"request.path".into())
            .unwrap_or(false));

        // accessing uses cache
        let method: Result<AttributeState<Option<String>>, _> = ctx.get_attribute("request.method");
        let path: Result<AttributeState<Option<String>>, _> = ctx.get_attribute("request.path");

        assert!(method.is_ok());
        assert!(path.is_ok());
    }

    #[test]
    fn test_request_data() {
        let mock_host = MockWasmHost::new()
            .with_property("auth.identity.user".into(), "alice".bytes().collect())
            .with_property("auth.identity.group".into(), "admin".bytes().collect());
        let backend = Arc::new(mock_host);

        let request_data = vec![
            (
                ("metrics.labels".to_string(), "user".to_string()),
                Expression::new("auth.identity.user").unwrap(),
            ),
            (
                ("metrics.labels".to_string(), "group".to_string()),
                Expression::new("auth.identity.group").unwrap(),
            ),
        ];

        // Without request_data
        let ctx_empty = ReqRespCtx::new(backend.clone());
        let results_empty = ctx_empty.eval_request_data();
        assert!(results_empty.is_empty());

        // With request_data
        let ctx = ReqRespCtx::new(backend).with_request_data(&request_data);
        let results = ctx.eval_request_data();
        assert_eq!(results.len(), 2);

        // Check metrics.labels.user result
        let user_result = results
            .iter()
            .find(|((domain, field), _)| domain == "metrics.labels" && field == "user");
        assert!(user_result.is_some());
        let (_, result) = user_result.unwrap();
        assert!(result.is_ok());
        if let Ok(AttributeState::Available(cel_interpreter::Value::String(user))) = result {
            assert_eq!(user.as_ref(), "alice");
        }

        // Check metrics.labels.group result
        let group_result = results
            .iter()
            .find(|((domain, field), _)| domain == "metrics.labels" && field == "group");
        assert!(group_result.is_some());
        let (_, result) = group_result.unwrap();
        assert!(result.is_ok());
        if let Ok(AttributeState::Available(cel_interpreter::Value::String(group))) = result {
            assert_eq!(group.as_ref(), "admin");
        }
    }
}
