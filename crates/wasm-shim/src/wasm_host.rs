use std::time::Duration;

use prost::Message;

use tracing::{debug, error};

use kuadrant_filter::data::attribute::{AttributeError, Path};
use kuadrant_filter::kuadrant::resolver::{AttributeResolver, MapType};
use kuadrant_filter::metrics::MetricsBackend;
use kuadrant_filter::services::ServiceError;
use proxy_wasm::hostcalls;
use proxy_wasm::types::{LogLevel, Status};
use tracing_subscriber::filter::LevelFilter;

pub fn current_log_filter() -> LevelFilter {
    match hostcalls::get_log_level() {
        Ok(LogLevel::Trace) => LevelFilter::TRACE,
        Ok(LogLevel::Debug) => LevelFilter::DEBUG,
        Ok(LogLevel::Info) => LevelFilter::INFO,
        Ok(LogLevel::Warn) => LevelFilter::WARN,
        Ok(LogLevel::Error) | Ok(LogLevel::Critical) => LevelFilter::ERROR,
        Err(_) => LevelFilter::WARN,
    }
}

pub struct ProxyWasmHost;

const PROTOBUF_VALUE_TYPE_URL: &str = "type.googleapis.com/google.protobuf.Value";

#[derive(Clone, PartialEq, prost::Message)]
struct DeclarePropertyArguments {
    #[prost(string, tag = "1")]
    name: String,
    #[prost(bool, tag = "2")]
    readonly: bool,
    #[prost(int32, tag = "3")]
    property_type: i32,
    #[prost(bytes = "vec", tag = "4")]
    schema: Vec<u8>,
    #[prost(int32, tag = "5")]
    life_span: i32,
}

fn protobuf_value_property_declaration(property_path: &str) -> DeclarePropertyArguments {
    DeclarePropertyArguments {
        name: format!("kuadrant.{property_path}"),
        readonly: false,
        property_type: 3, // envoy.source.extensions.common.wasm.WasmType.Protobuf
        schema: PROTOBUF_VALUE_TYPE_URL.as_bytes().to_vec(),
        life_span: 1, // envoy.source.extensions.common.wasm.LifeSpan.DownstreamRequest
    }
}

/// Declares an exported google.protobuf.Value property to Envoy.
pub fn declare_protobuf_value_property(property_path: &str) -> Result<(), String> {
    let arguments = protobuf_value_property_declaration(property_path);
    let encoded_arguments = arguments.encode_to_vec();
    hostcalls::call_foreign_function("declare_property", Some(&encoded_arguments))
        .map(|_| ())
        .map_err(|err| {
            format!(
                "failed to declare Envoy protobuf CEL state for `{}`: {:?}",
                arguments.name, err
            )
        })
}

pub struct WasmMetricsBackend;

impl MetricsBackend for WasmMetricsBackend {
    fn define_counter(&self, name: &str) -> Option<u32> {
        proxy_wasm::hostcalls::define_metric(proxy_wasm::types::MetricType::Counter, name).ok()
    }

    fn increment_counter(&self, id: u32, offset: i64) {
        let _ = proxy_wasm::hostcalls::increment_metric(id, offset);
    }
}

fn to_proxy_map_type(map_type: MapType) -> proxy_wasm::types::MapType {
    match map_type {
        MapType::HttpRequestHeaders => proxy_wasm::types::MapType::HttpRequestHeaders,
        MapType::HttpResponseHeaders => proxy_wasm::types::MapType::HttpResponseHeaders,
    }
}

impl AttributeResolver for ProxyWasmHost {
    fn get_attribute(&self, path: &Path) -> Result<Option<Vec<u8>>, AttributeError> {
        debug!("Getting property: `{}`", path);
        match hostcalls::get_property(path.tokens()) {
            Ok(data) => Ok(data),
            Err(proxy_wasm::types::Status::BadArgument) => Err(AttributeError::NotAvailable(
                format!("Property `{path}` not available in current request phase"),
            )),
            Err(e) => Err(AttributeError::Retrieval(format!(
                "failed to get property: `{path}`: {e:?}"
            ))),
        }
    }

    fn get_attribute_map(
        &self,
        map_type: MapType,
    ) -> Result<Vec<(String, String)>, AttributeError> {
        debug!("Getting map: `{:?}`", map_type);
        match hostcalls::get_map(to_proxy_map_type(map_type)) {
            Ok(map) if map.is_empty() => Err(AttributeError::NotAvailable(format!(
                "Map `{:?}` not available in current phase",
                map_type
            ))),
            Ok(map) => Ok(map),
            Err(err) => Err(AttributeError::Retrieval(format!(
                "Error getting host map: {err:?}"
            ))),
        }
    }

    fn get_attribute_map_value(
        &self,
        map_type: MapType,
        key: &str,
    ) -> Result<Option<String>, AttributeError> {
        debug!("Getting map value: `{:?}[{key}]`", map_type);
        match hostcalls::get_map_value(to_proxy_map_type(map_type), key) {
            Ok(value) => Ok(value),
            Err(Status::BadArgument) => Err(AttributeError::NotAvailable(format!(
                "Map `{map_type:?}` not available in current phase",
            ))),
            Err(err) => Err(AttributeError::Retrieval(format!(
                "Error getting map value: {err:?}"
            ))),
        }
    }

    fn set_attribute(&self, path: &Path, value: &[u8]) -> Result<(), AttributeError> {
        debug!("Setting property: `{}`", path);
        match hostcalls::set_property(path.tokens(), Some(value)) {
            Ok(_) => Ok(()),
            Err(err) => Err(AttributeError::Set(format!(
                "Failed to set property `{}`: {:?}",
                path, err
            ))),
        }
    }

    fn clear_attribute(&self, path: &Path) -> Result<(), AttributeError> {
        debug!("Clearing property: `{}`", path);
        match hostcalls::set_property(path.tokens(), None) {
            Ok(_) => Ok(()),
            Err(err) => Err(AttributeError::Set(format!(
                "Failed to clear property `{}`: {:?}",
                path, err
            ))),
        }
    }

    fn set_attribute_map(
        &self,
        map_type: MapType,
        value: Vec<(&str, &str)>,
    ) -> Result<(), AttributeError> {
        match hostcalls::set_map(to_proxy_map_type(map_type), value) {
            Ok(_) => Ok(()),
            Err(proxy_wasm::types::Status::BadArgument) => Err(AttributeError::NotAvailable(
                format!("Map `{:?}` not available in current phase", map_type),
            )),
            Err(err) => Err(AttributeError::Set(format!("Error setting map: {err:?}"))),
        }
    }

    fn get_http_request_body(
        &self,
        start: usize,
        max_size: usize,
    ) -> Result<Option<Vec<u8>>, AttributeError> {
        match hostcalls::get_buffer(
            proxy_wasm::types::BufferType::HttpRequestBody,
            start,
            max_size,
        ) {
            Ok(bytes) => Ok(bytes),
            Err(Status::BadArgument) => {
                Err(AttributeError::NotAvailable("request.body".to_string()))
            }
            Err(e) => Err(AttributeError::Retrieval(format!(
                "Error getting http request body buffer: {e:?}"
            ))),
        }
    }

    fn get_http_response_body(
        &self,
        start: usize,
        max_size: usize,
    ) -> Result<Option<Vec<u8>>, AttributeError> {
        match hostcalls::get_buffer(
            proxy_wasm::types::BufferType::HttpResponseBody,
            start,
            max_size,
        ) {
            Ok(bytes) => Ok(bytes),
            Err(Status::BadArgument) => {
                Err(AttributeError::NotAvailable("response.body".to_string()))
            }
            Err(e) => Err(AttributeError::Retrieval(format!(
                "Error getting http response body buffer: {e:?}"
            ))),
        }
    }

    fn dispatch_grpc_call(
        &self,
        upstream_name: &str,
        service_name: &str,
        method: &str,
        headers: Vec<(&str, &[u8])>,
        message: Vec<u8>,
        timeout: Duration,
    ) -> Result<u32, ServiceError> {
        debug!(
            "Dispatching gRPC call to {}/{}.{}, timeout: {:?}",
            upstream_name, service_name, method, timeout
        );
        match hostcalls::dispatch_grpc_call(
            upstream_name,
            service_name,
            method,
            headers,
            Some(&message),
            timeout,
        ) {
            Ok(token_id) => {
                debug!("gRPC call dispatched successfully, token_id: {}", token_id);
                Ok(token_id)
            }
            Err(e) => {
                error!(
                    "Failed to dispatch gRPC call to {}/{}.{}: {:?}",
                    upstream_name, service_name, method, e
                );
                Err(ServiceError::Dispatch(format!("{e:?}")))
            }
        }
    }

    fn get_grpc_response(&self, response_size: usize) -> Result<Vec<u8>, ServiceError> {
        if response_size == 0 {
            return Err(ServiceError::Retrieval(
                "Received response with size 0".to_string(),
            ));
        }
        debug!("Getting gRPC response, size: {} bytes", response_size);
        hostcalls::get_buffer(
            proxy_wasm::types::BufferType::GrpcReceiveBuffer,
            0,
            response_size,
        )
        .map_err(|e| ServiceError::Retrieval(format!("Failed to get gRPC response: {:?}", e)))?
        .ok_or_else(|| ServiceError::Retrieval("No gRPC response body available".to_string()))
    }

    fn send_http_reply(
        &self,
        status_code: u32,
        headers: Vec<(&str, &str)>,
        body: Option<&[u8]>,
    ) -> Result<(), ServiceError> {
        debug!("Sending local reply, status code: {}", status_code);
        hostcalls::send_http_response(status_code, headers, body)
            .map_err(|e| ServiceError::Dispatch(format!("Failed to send HTTP reply: {:?}", e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ocsf_property_declaration_uses_protobuf_value_and_request_lifetime() {
        let encoded = protobuf_value_property_declaration("custom.audit.metadata").encode_to_vec();
        let decoded = DeclarePropertyArguments::decode(encoded.as_slice())
            .expect("valid Envoy declare_property arguments");

        assert_eq!(decoded.name, "kuadrant.custom.audit.metadata");
        assert!(!decoded.readonly);
        assert_eq!(decoded.property_type, 3);
        assert_eq!(
            decoded.schema,
            PROTOBUF_VALUE_TYPE_URL.as_bytes().to_vec()
        );
        assert_eq!(decoded.life_span, 1);
    }
}
