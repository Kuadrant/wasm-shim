use cel::{Context, Env, Value};
use std::cell::{OnceCell, RefCell};
use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use tracing::{debug, error, warn};

use crate::data::attribute::{wasm_prop, AttributeError, AttributeState, AttributeValue, Path};
use crate::data::{Expression, Headers};
use crate::kuadrant::cache::{AttributeCache, CachedValue};
use crate::kuadrant::pipeline::tasks::Task;
use crate::kuadrant::resolver::AttributeResolver;
use crate::metrics::{noop_metrics, MetricsReporter};
use crate::services::ServiceError;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use uuid::Uuid;

const X_REQUEST_ID_HEADER: &str = "x-request-id";

type RequestData = ((String, String), Expression);

pub struct ReqRespCtx {
    backend: Arc<dyn AttributeResolver>,
    cache: Arc<AttributeCache>,
    request_data: Option<Vec<RequestData>>,
    pub request_body: BodyContext,
    pub response_body: BodyContext,
    grpc_response_data: Option<(u32, usize)>,
    tracing: TracingContext,
    tracker: Tracker,
    pub values: ValueStore,
    pub barrier: Barrier,
    pub cel: CelScope,
    metrics: Arc<dyn MetricsReporter>,
}

impl ReqRespCtx {
    pub fn new(backend: Arc<dyn AttributeResolver + 'static>) -> Self {
        Self {
            backend,
            cache: Arc::new(AttributeCache::new()),
            request_data: None,
            request_body: BodyContext::default(),
            response_body: BodyContext::default(),
            grpc_response_data: None,
            tracing: TracingContext::default(),
            tracker: Tracker::default(),
            values: ValueStore::default(),
            barrier: Barrier::default(),
            cel: CelScope::default(),
            metrics: noop_metrics(),
        }
    }

    pub fn with_metrics(mut self, metrics: Arc<dyn MetricsReporter>) -> Self {
        self.metrics = metrics;
        self
    }

    pub fn metrics(&self) -> &dyn MetricsReporter {
        self.metrics.as_ref()
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

    pub fn get_request_header(&self, key: &str) -> Option<String> {
        match self.backend.get_request_header_value(key) {
            Ok(value) => value,
            Err(e) => {
                warn!("failed to get request header '{key}': {e}");
                None
            }
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
                match self.backend.get_request_headers() {
                    Ok(vec) => Ok(CachedValue::Headers(vec.into())),
                    Err(AttributeError::NotAvailable(msg)) => {
                        // We cannot be Pending on request headers
                        Err(AttributeError::Retrieval(msg))
                    }
                    Err(e) => Err(e),
                }
            }
            ["response", "headers"] => {
                let vec = self.backend.get_response_headers()?;
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
                match self.backend.set_request_headers(value.to_vec()) {
                    Ok(()) => self.cache.insert(path.clone(), CachedValue::Headers(value)),
                    Err(AttributeError::NotAvailable(msg)) => {
                        // We cannot be Pending on request headers
                        Err(AttributeError::Set(msg))
                    }
                    Err(e) => Err(e),
                }
            }
            ["response", "headers"] => {
                self.backend.set_response_headers(value.to_vec())?;
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

    pub(crate) fn get_http_request_body(
        &self,
        start: usize,
        body_size: usize,
    ) -> Result<AttributeState<Option<Vec<u8>>>, AttributeError> {
        match self.backend.get_http_request_body(start, body_size) {
            Ok(maybe_bytes) => Ok(AttributeState::Available(maybe_bytes)),
            Err(AttributeError::NotAvailable(_)) => Ok(AttributeState::Pending),
            Err(e) => Err(e),
        }
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
        let mut headers: Vec<(&str, &[u8])> = tracing_headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_slice()))
            .collect();

        headers.push((X_REQUEST_ID_HEADER, self.request_id().as_bytes()));

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

        let context = if self.tracing.request_span_guard.is_some() {
            &tracing::Span::current().context()
        } else {
            &self.tracing.otel_context
        };

        opentelemetry::global::get_text_map_propagator(|propagator| {
            let mut injector = crate::tracing::HeadersInjector::new(&mut headers);
            propagator.inject_context(context, &mut injector);
        });

        headers
    }

    pub fn set_public_tracker_id(&mut self, id: String) {
        self.tracker.downstream_identifier = Some(id);
    }

    pub fn request_id(&self) -> &str {
        self.tracker.get_id(self)
    }

    pub fn tracker(&self) -> Option<(&str, &str)> {
        match &self.tracker.downstream_identifier {
            None => None,
            Some(id) => Some((id.as_str(), self.request_id())),
        }
    }
}

struct Tracker {
    id: OnceCell<String>,
    downstream_identifier: Option<String>,
}

impl Default for Tracker {
    fn default() -> Self {
        Self {
            id: OnceCell::new(),
            downstream_identifier: None,
        }
    }
}

impl Tracker {
    fn get_id(&self, ctx: &ReqRespCtx) -> &str {
        self.id.get_or_init(|| {
            if let Ok(AttributeState::Available(Some(headers))) =
                ctx.get_attribute::<Headers>("request.headers")
            {
                if let Some(header_id) = headers.get(X_REQUEST_ID_HEADER) {
                    debug!(
                        "found {} header in request headers, using as request id: {}",
                        X_REQUEST_ID_HEADER, header_id
                    );
                    return header_id.to_string();
                }
            }

            let generated_id = Uuid::new_v4().to_string();
            debug!("generated request id: {}", generated_id);
            generated_id
        })
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

#[derive(Debug, Default, Clone)]
pub struct Barrier {
    count: u32,
}

impl Barrier {
    pub fn raise(&mut self) {
        self.count += 1;
    }

    pub fn lower(&mut self) {
        match self.count.checked_sub(1) {
            Some(new_value) => self.count = new_value,
            None => {
                error!(
                    "Attempted to lower upstream barrier when count is already 0 - mismatched raise/lower pairs"
                );
            }
        }
    }

    pub fn is_tripped(&self) -> bool {
        self.count > 0
    }

    #[cfg(test)]
    pub fn count(&self) -> u32 {
        self.count
    }
}

#[derive(Default)]
pub struct BodyContext {
    buffer_size: usize,
    end_of_stream: bool,
    values: HashMap<String, Value>,
}

impl BodyContext {
    pub fn buffer_size(&self) -> usize {
        self.buffer_size
    }

    pub fn is_end_of_stream(&self) -> bool {
        self.end_of_stream
    }

    pub fn set_buffer_size(&mut self, size: usize, end_of_stream: bool) {
        self.buffer_size = size;
        self.end_of_stream = end_of_stream;
    }

    pub fn set_value<K: Into<String>, V: Into<Value>>(&mut self, key: K, value: V) {
        self.values.insert(key.into(), value.into());
    }

    pub fn get_value(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }
}

pub struct CelScope {
    env: Arc<Env>,
    registered: HashSet<String>,
    bindings: BTreeMap<String, Vec<(String, Value)>>,
}

impl Default for CelScope {
    fn default() -> Self {
        Self {
            env: Arc::new(Env::stdlib()),
            registered: HashSet::new(),
            bindings: BTreeMap::new(),
        }
    }
}

impl CelScope {
    pub fn new_ctx(&mut self, task: &dyn Task) -> Context<'static> {
        let task_id = task.id();

        if !self.registered.contains(task_id) {
            let types = task.cel_types();
            if types.is_empty() {
                self.registered.insert(task_id.to_string());
            } else if let Some(env) = Arc::get_mut(&mut self.env) {
                for type_def in types {
                    env.add_struct(type_def);
                }
                self.registered.insert(task_id.to_string());
            } else {
                error!("Failed to add CEL types: Arc refcount > 1");
            }
        }

        let mut ctx = Context::with_env(Arc::clone(&self.env));
        for (scope_id, scope_bindings) in &self.bindings {
            if is_ancestor(scope_id, task_id) {
                for (name, value) in scope_bindings {
                    ctx.add_variable_from_value(name, value.clone());
                }
            }
        }
        ctx
    }

    pub fn add_scoped_binding(&mut self, task_id: &str, name: String, val: Value) {
        self.bindings
            .entry(task_id.to_string())
            .or_default()
            .push((name, val));
    }
}

fn is_ancestor(scope_id: &str, task_id: &str) -> bool {
    task_id == scope_id || task_id.starts_with(&format!("{}.", scope_id))
}

pub struct PathReservation {
    path: String,
    registry: Rc<RefCell<HashMap<String, usize>>>,
}

impl PathReservation {
    fn new(path: String, registry: &Rc<RefCell<HashMap<String, usize>>>) -> Self {
        *registry.borrow_mut().entry(path.clone()).or_insert(0) += 1;
        Self {
            path,
            registry: Rc::clone(registry),
        }
    }
}

impl Drop for PathReservation {
    fn drop(&mut self) {
        let mut map = self.registry.borrow_mut();
        match map.entry(self.path.clone()) {
            Entry::Occupied(mut entry) => {
                *entry.get_mut() -= 1;
                if *entry.get() == 0 {
                    entry.remove();
                }
            }
            Entry::Vacant(_) => {
                error!(
                    "PathReservation dropped for '{}' but no reservation found",
                    self.path
                );
            }
        }
    }
}

#[derive(Default)]
pub struct ValueStore {
    values: BTreeMap<String, Value>,
    reserved: Rc<RefCell<HashMap<String, usize>>>,
}

impl ValueStore {
    pub fn get(&self, path: &str) -> Option<&Value> {
        self.values.get(path)
    }

    pub fn is_pending(&self, path: &str) -> bool {
        !self.values.contains_key(path) && self.reserved.borrow().contains_key(path)
    }

    pub fn has_prefix(&self, prefix: &str) -> bool {
        self.values
            .range::<String, _>(prefix.to_string()..)
            .next()
            .is_some_and(|(k, _)| k.starts_with(prefix))
            || self.reserved.borrow().keys().any(|p| p.starts_with(prefix))
    }

    pub fn store(&mut self, path: String, value: Value) {
        self.values.insert(path, value);
    }

    pub fn reserve(&self, path: String) -> PathReservation {
        PathReservation::new(path, &self.reserved)
    }

    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.values.keys().map(|s| s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::attribute::AttributeState;
    use crate::kuadrant::resolver::MockWasmHost;
    use std::sync::Arc;

    #[test]
    fn test_barrier_starts_not_tripped() {
        let barrier = Barrier::default();
        assert_eq!(barrier.count(), 0);
        assert!(!barrier.is_tripped());
    }

    #[test]
    fn test_barrier_raise_and_lower() {
        let mut barrier = Barrier::default();

        assert_eq!(barrier.count(), 0);
        assert!(!barrier.is_tripped());

        barrier.raise();
        assert_eq!(barrier.count(), 1);
        assert!(barrier.is_tripped());

        barrier.raise();
        assert_eq!(barrier.count(), 2);
        assert!(barrier.is_tripped());

        barrier.lower();
        assert_eq!(barrier.count(), 1);
        assert!(barrier.is_tripped());

        barrier.lower();
        assert_eq!(barrier.count(), 0);
        assert!(!barrier.is_tripped());
    }

    #[test]
    fn test_barrier_underflow_protection() {
        let mut barrier = Barrier::default();

        assert_eq!(barrier.count(), 0);
        assert!(!barrier.is_tripped());

        // Attempting to lower when already at 0 should log error and remain at 0
        barrier.lower();
        assert_eq!(barrier.count(), 0);
        assert!(!barrier.is_tripped());

        // Verify multiple underflow attempts don't cause issues
        barrier.lower();
        assert_eq!(barrier.count(), 0);
        assert!(!barrier.is_tripped());

        // Normal operation should still work after underflow
        barrier.raise();
        assert_eq!(barrier.count(), 1);
        assert!(barrier.is_tripped());
    }

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
        ];

        let mock_host = MockWasmHost::new().with_map("request.headers".to_string(), headers);
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));
        ctx.extract_trace_context();

        let tracing_headers = ctx.get_tracing_headers();

        assert_eq!(tracing_headers.len(), 3);

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

        assert_eq!(tracing_headers.len(), 1);
        assert_eq!(tracing_headers[0].0, "traceparent");
        assert_eq!(
            tracing_headers[0].1.as_slice(),
            b"00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
        );
    }

    #[test]
    fn test_tracing_headers_none() {
        let headers: Vec<(String, String)> = vec![];

        let mock_host = MockWasmHost::new().with_map("request.headers".to_string(), headers);
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));
        ctx.extract_trace_context();

        let tracing_headers = ctx.get_tracing_headers();

        assert!(tracing_headers.is_empty());
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

    #[test]
    fn test_cel_scope_hierarchical_bindings() {
        use crate::kuadrant::pipeline::tasks::{Task, TaskOutcome};

        struct MockTask {
            id: String,
        }
        impl Task for MockTask {
            fn id(&self) -> &str {
                &self.id
            }
            fn apply(self: Box<Self>, _ctx: &mut ReqRespCtx) -> TaskOutcome {
                TaskOutcome::Done
            }
        }

        let mut scope = CelScope::default();

        // Task "0" gets a binding
        scope.add_scoped_binding(
            "0",
            "my_response".to_string(),
            Value::String(Arc::new("user123".to_string())),
        );

        // Task "0.0" should see "my_response" from parent "0"
        let task_0_0 = MockTask {
            id: "0.0".to_string(),
        };
        let ctx_0_0 = scope.new_ctx(&task_0_0);
        assert!(ctx_0_0.get_variable("my_response").is_some());

        // Task "1" should NOT see "my_response" (different branch)
        let task_1 = MockTask {
            id: "1".to_string(),
        };
        let ctx_1 = scope.new_ctx(&task_1);
        assert!(ctx_1.get_variable("my_response").is_none());
    }

    #[test]
    fn test_is_ancestor() {
        // Same task
        assert!(is_ancestor("0", "0"));

        // Direct child
        assert!(is_ancestor("0", "0.0"));

        // Nested child
        assert!(is_ancestor("0", "0.0.1"));

        // Different branch
        assert!(!is_ancestor("0", "1"));
        assert!(!is_ancestor("0", "1.0"));

        // Sibling
        assert!(!is_ancestor("0.0", "0.1"));

        // Parent relationship is not symmetric
        assert!(!is_ancestor("0.0", "0"));
    }

    #[test]
    fn reservation_registers_and_clears_on_drop() {
        let ctx = ReqRespCtx::new(Arc::new(MockWasmHost::new()));

        {
            let _reservation = ctx.values.reserve("auth.complete".to_string());
            assert!(ctx.values.is_pending("auth.complete"));
        }

        assert!(!ctx.values.is_pending("auth.complete"));
        assert!(ctx.values.get("auth.complete").is_none());
    }

    #[test]
    fn multiple_reservations_independent_lifecycle() {
        let ctx = ReqRespCtx::new(Arc::new(MockWasmHost::new()));

        let r1 = ctx.values.reserve("a.b".to_string());
        let _r2 = ctx.values.reserve("x.y".to_string());

        assert!(ctx.values.is_pending("a.b"));
        assert!(ctx.values.is_pending("x.y"));

        drop(r1);
        assert!(!ctx.values.is_pending("a.b"));
        assert!(ctx.values.is_pending("x.y"));
    }

    #[test]
    fn has_prefix_matches_reserved_descendants() {
        let ctx = ReqRespCtx::new(Arc::new(MockWasmHost::new()));
        let _reservation = ctx.values.reserve("auth.identity.username".to_string());

        assert!(ctx.values.has_prefix("auth."));
        assert!(ctx.values.has_prefix("auth.identity."));
        assert!(!ctx.values.has_prefix("other."));
        assert!(!ctx.values.is_pending("auth"));
    }
}
