extern crate core;

#[allow(unused_imports)]
mod envoy;
mod v2;

#[cfg_attr(
    all(
        target_arch = "wasm32",
        target_vendor = "unknown",
        target_os = "unknown"
    ),
    export_name = "_start"
)]
#[cfg_attr(
    not(all(
        target_arch = "wasm32",
        target_vendor = "unknown",
        target_os = "unknown"
    )),
    allow(dead_code)
)]
// This is a C interface, so make it explicit in the fn signature (and avoid mangling)
extern "C" fn start() {
    use crate::v2::filter::FilterRoot;
    use log::info;
    use proxy_wasm::traits::RootContext;
    use proxy_wasm::types::LogLevel;

    proxy_wasm::set_log_level(LogLevel::Trace);
    std::panic::set_hook(Box::new(|panic_info| {
        let _ = proxy_wasm::hostcalls::log(LogLevel::Critical, &panic_info.to_string());
    }));
    proxy_wasm::set_root_context(|context_id| -> Box<dyn RootContext> {
        info!("#{} set_root_context", context_id);
        Box::new(FilterRoot::new(context_id))
    });
}

#[cfg(test)]
mod tests {
    use crate::envoy::{rate_limit_response, HeaderValue, RateLimitResponse};
    use prost::Message;

    #[test]
    fn grpc() {
        let resp = RateLimitResponse {
            overall_code: rate_limit_response::Code::Ok as i32,
            statuses: Vec::new(),
            response_headers_to_add: vec![
                header("test", "some value"),
                header("other", "header value"),
            ],
            request_headers_to_add: Vec::new(),
            raw_body: Vec::new(),
            dynamic_metadata: None,
            quota: None,
        };
        let buffer = resp.encode_to_vec();
        let expected: [u8; 45] = [
            8, 1, 26, 18, 10, 4, 116, 101, 115, 116, 18, 10, 115, 111, 109, 101, 32, 118, 97, 108,
            117, 101, 26, 21, 10, 5, 111, 116, 104, 101, 114, 18, 12, 104, 101, 97, 100, 101, 114,
            32, 118, 97, 108, 117, 101,
        ];
        assert_eq!(expected, buffer.as_slice())
    }

    fn header(key: &str, value: &str) -> HeaderValue {
        HeaderValue {
            key: key.to_string(),
            value: value.to_string(),
        }
    }
}
