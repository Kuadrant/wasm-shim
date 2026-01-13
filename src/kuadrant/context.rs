use cel_interpreter::Value;
use log::{debug, warn};
use std::cell::{LazyCell, OnceCell};
use std::collections::HashMap;
use std::sync::Arc;

use crate::data::attribute::{wasm_prop, AttributeError, AttributeState, AttributeValue, Path};
use crate::data::{Expression, Headers};
use crate::kuadrant::cache::{AttributeCache, CachedValue};
use crate::kuadrant::resolver::{AttributeResolver, ProxyWasmHost};
use crate::services::ServiceError;
use crate::X_REQUEST_ID_HEADER;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use uuid::Uuid;

type RequestData = ((String, String), Expression);

pub struct ReqRespCtx {
    backend: Arc<dyn AttributeResolver>,
    cache: Arc<AttributeCache>,
    request_data: Option<Vec<RequestData>>,
    response_body_size: usize,
    response_end_of_stream: bool,
    // todo(refactor): we should handle token here
    grpc_response_data: Option<(u32, usize)>,
    tracing: TracingContext,
    tracker: Tracker,
    body_values: HashMap<String, Value>,
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
            response_body_size: 0,
            response_end_of_stream: false,
            grpc_response_data: None,
            tracing: TracingContext::default(),
            tracker: Tracker::default(),
            body_values: HashMap::new(),
        }
    }

    pub fn with_request_data(mut self, request_data: Vec<RequestData>) -> Self {
        self.request_data = Some(request_data);
        self
    }

    pub fn extract_trace_context(&mut self) {
        let request_headers: Result<AttributeState<Option<Headers>>, _> =
            self.get_attribute("request.headers");

        if let Ok(AttributeState::Available(Some(header_map))) = request_headers {
            let extractor = crate::tracing::HeadersExtractor::new(&header_map);
            self.tracing.otel_context =
                opentelemetry::global::get_text_map_propagator(|propagator| {
                    propagator.extract(&extractor)
                });
        }
    }

    pub fn enter_request_span(&mut self) {
        let span = tracing::info_span!(
            "kuadrant_filter",
            action_set = tracing::field::Empty,
            hostname = tracing::field::Empty,
            request_id = tracing::field::Empty
        );
        if !span.is_disabled() {
            if let Err(e) = span.set_parent(self.tracing.otel_context.clone()) {
                debug!("failed to set parent span ctx: {e:?}");
            }
            span.record("request_id", self.request_id());
            if let Some(action_set) = &self.tracing.action_set_name {
                span.record("action_set", action_set.as_str());
            }
            if let Some(hostname) = &self.tracing.hostname {
                span.record("hostname", hostname.as_str());
            }
            self.tracing.request_span_guard = Some(span.entered());
        }
    }

    pub fn end_request_span(&mut self) {
        std::mem::drop(self.tracing.request_span_guard.take());
    }

    pub fn set_action_set_name(&mut self, name: String) {
        self.tracing.action_set_name = Some(name);
    }

    pub fn set_hostname(&mut self, hostname: String) {
        self.tracing.hostname = Some(hostname);
    }

    pub fn set_current_response_body_buffer_size(&mut self, body_size: usize, end_of_stream: bool) {
        self.response_body_size = body_size;
        self.response_end_of_stream = end_of_stream;
    }

    pub fn set_grpc_response_data(
        &mut self,
        status_code: u32,
        response_size: usize,
    ) -> Result<(), ServiceError> {
        if self.grpc_response_data.is_some() {
            return Err(ServiceError::Retrieval(
                "gRPC response data already set".to_string(),
            ));
        }
        self.grpc_response_data = Some((status_code, response_size));
        Ok(())
    }

    pub fn get_grpc_response_data(&mut self) -> Result<(u32, usize), ServiceError> {
        self.grpc_response_data
            .take()
            .ok_or_else(|| ServiceError::Retrieval("No gRPC response data available".to_string()))
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
        // We store in the filter escaped with the kuadrant namespace
        // but in the cache it remains unchanged
        const KUADRANT_NAMESPACE: &str = "kuadrant";

        let escaped_path = path.tokens().join("\\.");
        let host_path = format!("{}\\.{}", KUADRANT_NAMESPACE, escaped_path);

        self.backend
            .set_attribute(&host_path.as_str().into(), &value)?;
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
        match self.store_attribute_bytes(&attribute_path.into(), value.to_vec()) {
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
        self.response_end_of_stream
    }

    pub fn response_body_buffer_size(&self) -> usize {
        self.response_body_size
    }

    pub(crate) fn get_http_response_body(
        &self,
        start: usize,
        body_size: usize,
    ) -> Result<AttributeState<Option<Vec<u8>>>, AttributeError> {
        match self.backend.get_http_response_body(start, body_size) {
            Ok(maybe_bytes) => Ok(AttributeState::Available(maybe_bytes)),
            Err(AttributeError::NotAvailable(_)) => Ok(AttributeState::Pending),
            Err(e) => Err(e),
        }
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
            .map(|(name, value)| (name.as_str(), value.as_slice()))
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

    pub fn send_http_reply(
        &self,
        status_code: u32,
        headers: Vec<(&str, &str)>,
        body: Option<&[u8]>,
    ) -> Result<(), ServiceError> {
        self.backend.send_http_reply(status_code, headers, body)
    }

    fn get_tracing_headers(&self) -> Vec<(String, Vec<u8>)> {
        let mut headers = Vec::new();

        let context = if tracing::Span::current().is_none() {
            &self.tracing.otel_context
        } else {
            &tracing::Span::current().context()
        };

        opentelemetry::global::get_text_map_propagator(|propagator| {
            let mut injector = crate::tracing::HeadersInjector::new(&mut headers);
            propagator.inject_context(context, &mut injector);
        });

        headers.push((
            X_REQUEST_ID_HEADER.to_string(),
            self.request_id().as_bytes().to_vec(),
        ));

        headers
    }

    pub fn set_public_tracker_id(&mut self, id: String) {
        self.tracker.downstream_identifier = Some(id);
    }

    pub fn request_id(&self) -> &str {
        self.tracker
            .get_header_request_id(self)
            .unwrap_or_else(|| self.tracker.generated_id.as_str())
    }

    pub fn tracker(&self) -> Option<(&str, &str)> {
        match &self.tracker.downstream_identifier {
            None => None,
            Some(id) => Some((id.as_str(), self.request_id())),
        }
    }

    pub fn set_body_value<K: Into<String>, V: Into<Value>>(&mut self, key: K, value: V) {
        self.body_values.insert(key.into(), value.into());
    }

    pub fn get_body_value(&self, key: &str) -> Option<&Value> {
        self.body_values.get(key)
    }
}

struct Tracker {
    header_request_id: OnceCell<Option<String>>,
    generated_id: LazyCell<String>,
    downstream_identifier: Option<String>,
}

impl Default for Tracker {
    fn default() -> Self {
        Self {
            header_request_id: OnceCell::new(),
            generated_id: LazyCell::new(|| Uuid::new_v4().to_string()),
            downstream_identifier: None,
        }
    }
}

impl Tracker {
    fn get_header_request_id(&self, ctx: &ReqRespCtx) -> Option<&str> {
        self.header_request_id
            .get_or_init(|| match ctx.get_attribute::<Headers>("request.headers") {
                Ok(AttributeState::Available(Some(headers))) => {
                    let header_id = headers.get(X_REQUEST_ID_HEADER).map(|v| v.to_string());
                    if let Some(ref x_request_id) = header_id {
                        debug!(
                            "found {} header in request headers, using as request id: {}",
                            X_REQUEST_ID_HEADER, x_request_id
                        );
                    }
                    header_id
                }
                _ => None,
            })
            .as_deref()
    }
}

struct TracingContext {
    otel_context: opentelemetry::Context,
    request_span_guard: Option<tracing::span::EnteredSpan>,
    action_set_name: Option<String>,
    hostname: Option<String>,
}

impl Default for TracingContext {
    fn default() -> Self {
        Self {
            otel_context: opentelemetry::Context::new(),
            request_span_guard: None,
            action_set_name: None,
            hostname: None,
        }
    }
}

pub mod request_data {
    use crate::data::cel::EvalResult;

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
    use crate::data::attribute::AttributeState;
    use crate::data::cel::Expression;
    use crate::kuadrant::resolver::MockWasmHost;
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
        let ctx = ReqRespCtx::new(backend).with_request_data(request_data);
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
            (
                "traceparent".to_string(),
                "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".to_string(),
            ),
            ("tracestate".to_string(), "state=active".to_string()),
            ("baggage".to_string(), "userId=alice".to_string()),
            ("baggage".to_string(), "sessionId=xyz".to_string()),
            ("content-type".to_string(), "application/json".to_string()),
            (
                "x-request-id".to_string(),
                "e1fc297a-a8a3-4360-8f41-af57b4a861e1".to_string(),
            ),
        ];

        let mock_host = MockWasmHost::new().with_map("request.headers".to_string(), headers);
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));
        ctx.extract_trace_context();

        let tracing_headers = ctx.get_tracing_headers();

        assert_eq!(tracing_headers.len(), 4);

        assert!(tracing_headers
            .iter()
            .any(|(name, value)| *name == "traceparent"
                && value.as_slice() == b"00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"));
        assert!(tracing_headers
            .iter()
            .any(|(name, value)| *name == "tracestate" && value.as_slice() == b"state=active"));

        let baggage = tracing_headers
            .iter()
            .find(|(name, _)| *name == "baggage")
            .expect("baggage header should exist");

        let baggage_value = std::str::from_utf8(&baggage.1).expect("valid UTF-8");
        assert!(baggage_value.contains("userId=alice"));
        assert!(baggage_value.contains("sessionId=xyz"));

        assert!(tracing_headers
            .iter()
            .any(|(name, value)| *name == "x-request-id"
                && value.as_slice() == b"e1fc297a-a8a3-4360-8f41-af57b4a861e1"));

        assert!(!tracing_headers
            .iter()
            .any(|(name, _)| *name == "content-type"));
    }

    #[test]
    fn test_tracing_headers_partial() {
        let headers = vec![(
            "traceparent".to_string(),
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".to_string(),
        )];

        let mock_host = MockWasmHost::new().with_map("request.headers".to_string(), headers);
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));
        ctx.extract_trace_context();

        let tracing_headers = ctx.get_tracing_headers();

        assert_eq!(tracing_headers.len(), 2);

        assert!(tracing_headers
            .iter()
            .any(|(name, value)| *name == "traceparent"
                && value.as_slice() == b"00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"));

        assert!(tracing_headers
            .iter()
            .any(|(name, _)| *name == "x-request-id"));
    }

    #[test]
    fn test_tracing_headers_none() {
        let headers: Vec<(String, String)> = vec![];

        let mock_host = MockWasmHost::new().with_map("request.headers".to_string(), headers);
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));
        ctx.extract_trace_context();

        let tracing_headers = ctx.get_tracing_headers();

        assert_eq!(tracing_headers.len(), 1);
        assert!(tracing_headers
            .iter()
            .any(|(name, _)| *name == "x-request-id"));
    }

    #[test]
    fn test_set_attribute_cache_consistency() {
        let mock_host = MockWasmHost::new();
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let value = b"test-user-id";
        ctx.set_attribute("auth.identity.userid", value).unwrap();

        assert!(ctx
            .cache
            .contains_key(&"auth.identity.userid".into())
            .unwrap());

        let result: Result<AttributeState<Option<String>>, _> =
            ctx.get_attribute("auth.identity.userid");
        assert!(matches!(
            result,
            Ok(AttributeState::Available(Some(ref s))) if s == "test-user-id"
        ));
    }

    #[test]
    fn test_get_attribute_from_host_when_not_in_cache() {
        let mock_host = MockWasmHost::new().with_property(
            Path::new(vec!["filter_state", "wasm.kuadrant.auth.identity.userid"]),
            b"external-user-id".to_vec(),
        );
        let ctx = ReqRespCtx::new(Arc::new(mock_host));

        let cache_path: Path = "auth.identity.userid".into();
        assert!(!ctx.cache.contains_key(&cache_path).unwrap());

        let result: Result<AttributeState<Option<String>>, _> =
            ctx.get_attribute("auth.identity.userid");
        assert!(matches!(
            result,
            Ok(AttributeState::Available(Some(ref s))) if s == "external-user-id"
        ));

        assert!(ctx.cache.contains_key(&cache_path).unwrap());

        let result2: Result<AttributeState<Option<String>>, _> =
            ctx.get_attribute("auth.identity.userid");
        assert!(matches!(
            result2,
            Ok(AttributeState::Available(Some(ref s))) if s == "external-user-id"
        ));
    }
}
