use log::warn;
use proxy_wasm::types::Bytes;
use std::sync::Arc;

use crate::v2::data::attribute::{wasm_prop, AttributeError, AttributeState, AttributeValue, Path};
use crate::v2::data::{Expression, Headers};
use crate::v2::kuadrant::cache::{AttributeCache, CachedValue};
use crate::v2::kuadrant::resolver::{AttributeResolver, ProxyWasmHost};
use crate::v2::services::ServiceError;

type RequestData = ((String, String), Expression);

#[derive(Clone)]
pub struct ReqRespCtx {
    backend: Arc<dyn AttributeResolver>,
    cache: Arc<AttributeCache>,
    request_data: Option<Arc<Vec<RequestData>>>,
    body_size: usize,
    end_of_stream: bool,
}

impl Default for ReqRespCtx {
    fn default() -> Self {
        Self::new(Arc::new(ProxyWasmHost))
    }
}

impl ReqRespCtx {
    pub fn new(backend: Arc<dyn AttributeResolver + 'static>) -> Self {
        Self {
            backend,
            cache: Arc::new(AttributeCache::new()),
            request_data: None,
            body_size: 0,
            end_of_stream: false,
        }
    }

    pub fn with_request_data(mut self, request_data: Arc<Vec<RequestData>>) -> Self {
        self.request_data = Some(request_data);
        self
    }

    pub fn with_body_size(mut self, body_size: usize) -> Self {
        self.body_size = body_size;
        self
    }

    pub fn with_end_of_stream(mut self, end_of_stream: bool) -> Self {
        self.end_of_stream = end_of_stream;
        self
    }

    pub fn get_attribute<T: AttributeValue>(
        &self,
        path: impl Into<Path>,
    ) -> Result<AttributeState<Option<T>>, AttributeError> {
        self.get_attribute_ref(&path.into())
    }

    pub fn get_required<T: AttributeValue>(
        &self,
        path: impl Into<Path>,
    ) -> Result<T, AttributeError> {
        let path = path.into();
        match self.get_attribute_ref::<T>(&path)? {
            AttributeState::Available(Some(value)) => Ok(value),
            AttributeState::Available(None) => {
                Err(AttributeError::Retrieval(format!("{} not set", path)))
            }
            AttributeState::Pending => Err(AttributeError::NotAvailable(format!(
                "{} still pending",
                path
            ))),
        }
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
                match self
                    .backend
                    .get_attribute_map(proxy_wasm::types::MapType::HttpRequestHeaders)
                {
                    Ok(vec) => Ok(CachedValue::Headers(vec.into())),
                    Err(AttributeError::NotAvailable(msg)) => {
                        // We cannot be Pending on request headers
                        Err(AttributeError::Retrieval(msg))
                    }
                    Err(e) => Err(e),
                }
            }
            ["response", "headers"] => {
                let vec = self
                    .backend
                    .get_attribute_map(proxy_wasm::types::MapType::HttpResponseHeaders)?;
                Ok(CachedValue::Headers(vec.into()))
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

    fn store_attribute_bytes(&self, path: &Path, value: Vec<u8>) -> Result<(), AttributeError> {
        self.backend.set_attribute(path, &value)?;
        self.cache
            .insert(path.clone(), CachedValue::Bytes(Some(value)))
    }

    fn store_attribute_headers(&self, path: &Path, value: Headers) -> Result<(), AttributeError> {
        match *path.tokens() {
            ["request", "headers"] => {
                match self.backend.set_attribute_map(
                    proxy_wasm::types::MapType::HttpRequestHeaders,
                    value.to_vec(),
                ) {
                    Ok(()) => self.cache.insert(path.clone(), CachedValue::Headers(value)),
                    Err(AttributeError::NotAvailable(msg)) => {
                        // We cannot be Pending on request headers
                        Err(AttributeError::Set(msg))
                    }
                    Err(e) => Err(e),
                }
            }
            ["response", "headers"] => {
                self.backend.set_attribute_map(
                    proxy_wasm::types::MapType::HttpResponseHeaders,
                    value.to_vec(),
                )?;
                self.cache.insert(path.clone(), CachedValue::Headers(value))
            }
            _ => Err(AttributeError::Set(
                "Headers can only be set on request.headers or response.headers".to_string(),
            )),
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

    pub fn eval_request_data(&self) -> Vec<request_data::RequestDataEntry> {
        let Some(ref expressions) = self.request_data else {
            return Vec::new();
        };
        expressions
            .iter()
            .map(|((domain, field), expr)| request_data::RequestDataEntry {
                domain: domain.clone(),
                field: field.clone(),
                result: expr.eval(self),
                source: expr.to_string(),
            })
            .collect()
    }

    pub fn set_attribute(
        &self,
        attribute_path: &str,
        value: &[u8],
    ) -> Result<AttributeState<()>, AttributeError> {
        const KUADRANT_NAMESPACE: &str = "kuadrant";

        let escaped_path = attribute_path.replace('.', "\\.");
        let full_path = format!("{}\\.{}", KUADRANT_NAMESPACE, escaped_path);
        let path = Path::from(full_path.as_str());

        match self.store_attribute_bytes(&path, value.to_vec()) {
            Ok(()) => Ok(AttributeState::Available(())),
            Err(AttributeError::NotAvailable(_)) => Ok(AttributeState::Pending),
            Err(e) => Err(e),
        }
    }

    /// Sets header maps for request or response headers
    pub fn set_attribute_map(
        &self,
        path: &Path,
        value: Headers,
    ) -> Result<AttributeState<()>, AttributeError> {
        match self.store_attribute_headers(path, value) {
            Ok(()) => Ok(AttributeState::Available(())),
            Err(AttributeError::NotAvailable(_)) => Ok(AttributeState::Pending),
            Err(e) => Err(e),
        }
    }

    pub fn is_end_of_stream(&self) -> bool {
        self.end_of_stream
    }

    pub fn body_size(&self) -> usize {
        self.body_size
    }

    pub(crate) fn get_http_response_body(
        &self,
        start: usize,
        body_size: usize,
    ) -> Result<Option<Bytes>, AttributeError> {
        self.backend.get_http_response_body(start, body_size)
    }

    pub fn dispatch_grpc_call(
        &self,
        upstream_name: &str,
        service_name: &str,
        method: &str,
        message: Vec<u8>,
        timeout: std::time::Duration,
    ) -> Result<u32, ServiceError> {
        let tracing_headers = self.get_tracing_headers();
        let headers: Vec<(&str, &[u8])> = tracing_headers
            .iter()
            .map(|(name, value)| (*name, value.as_slice()))
            .collect();

        self.backend.dispatch_grpc_call(
            upstream_name,
            service_name,
            method,
            headers,
            message,
            timeout,
        )
    }

    pub fn get_grpc_response(&self, response_size: usize) -> Result<Vec<u8>, ServiceError> {
        self.backend.get_grpc_response(response_size)
    }

    fn get_tracing_headers(&self) -> Vec<(&'static str, Vec<u8>)> {
        const TRACING_HEADERS: [&str; 3] = ["traceparent", "tracestate", "baggage"];
        let mut headers = Vec::new();

        let request_headers: Result<AttributeState<Option<Headers>>, _> =
            self.get_attribute("request.headers");

        if let Ok(AttributeState::Available(Some(header_map))) = request_headers {
            for header_name in &TRACING_HEADERS {
                for header_value in header_map.get_all(header_name) {
                    headers.push((*header_name, header_value.as_bytes().to_vec()));
                }
            }
        }

        headers
    }
}

pub mod request_data {
    use crate::v2::data::cel::EvalResult;

    pub struct RequestDataEntry {
        pub domain: String,
        pub field: String,
        pub result: EvalResult,
        pub source: String,
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
        let ctx = ReqRespCtx::new(backend).with_request_data(Arc::new(request_data));
        let results = ctx.eval_request_data();
        assert_eq!(results.len(), 2);

        // Check metrics.labels.user result
        let user_result = results
            .iter()
            .find(|entry| entry.domain == "metrics.labels" && entry.field == "user");
        assert!(user_result.is_some());
        let entry = user_result.unwrap();
        assert!(entry.result.is_ok());
        if let Ok(AttributeState::Available(cel_interpreter::Value::String(user))) = &entry.result {
            assert_eq!(user.as_ref(), "alice");
        }

        // Check metrics.labels.group result
        let group_result = results
            .iter()
            .find(|entry| entry.domain == "metrics.labels" && entry.field == "group");
        assert!(group_result.is_some());
        let entry = group_result.unwrap();
        assert!(entry.result.is_ok());
        if let Ok(AttributeState::Available(cel_interpreter::Value::String(group))) = &entry.result
        {
            assert_eq!(group.as_ref(), "admin");
        }
    }

    #[test]
    fn test_tracing_headers() {
        let headers = vec![
            ("traceparent".to_string(), "00-trace-id-123".to_string()),
            ("tracestate".to_string(), "state=active".to_string()),
            ("baggage".to_string(), "userId=alice".to_string()),
            ("baggage".to_string(), "sessionId=xyz".to_string()),
            ("content-type".to_string(), "application/json".to_string()),
        ];

        let mock_host = MockWasmHost::new().with_map("request.headers".to_string(), headers);
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let tracing_headers = ctx.get_tracing_headers();

        assert_eq!(tracing_headers.len(), 4);

        assert!(tracing_headers
            .iter()
            .any(|(name, value)| *name == "traceparent" && value.as_slice() == b"00-trace-id-123"));
        assert!(tracing_headers
            .iter()
            .any(|(name, value)| *name == "tracestate" && value.as_slice() == b"state=active"));

        let baggage_values: Vec<_> = tracing_headers
            .iter()
            .filter(|(name, _)| *name == "baggage")
            .collect();
        assert_eq!(baggage_values.len(), 2);
        assert!(baggage_values
            .iter()
            .any(|(_, value)| value.as_slice() == b"userId=alice"));
        assert!(baggage_values
            .iter()
            .any(|(_, value)| value.as_slice() == b"sessionId=xyz"));

        assert!(!tracing_headers
            .iter()
            .any(|(name, _)| *name == "content-type"));
    }

    #[test]
    fn test_tracing_headers_partial() {
        let headers = vec![("traceparent".to_string(), "00-trace-id-456".to_string())];

        let mock_host = MockWasmHost::new().with_map("request.headers".to_string(), headers);
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let tracing_headers = ctx.get_tracing_headers();

        assert_eq!(tracing_headers.len(), 1);
        assert_eq!(tracing_headers[0].0, "traceparent");
        assert_eq!(tracing_headers[0].1.as_slice(), b"00-trace-id-456");
    }

    #[test]
    fn test_tracing_headers_none() {
        let headers: Vec<(String, String)> = vec![];

        let mock_host = MockWasmHost::new().with_map("request.headers".to_string(), headers);
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let tracing_headers = ctx.get_tracing_headers();

        assert!(tracing_headers.is_empty());
    }
}
