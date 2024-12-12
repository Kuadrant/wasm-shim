mod action_set_index;
mod auth_action;
mod configuration;
mod data;
#[allow(renamed_and_removed_lints)]
mod envoy;
mod filter;
mod glob;

#[allow(dead_code)]
mod operation_dispatcher;
mod ratelimit_action;
mod runtime_action;
mod runtime_action_set;
mod runtime_config;
mod service;

#[cfg(test)]
mod tests {
    use crate::envoy::{HeaderValue, RateLimitResponse, RateLimitResponse_Code};
    use protobuf::Message;

    #[test]
    fn grpc() {
        let mut resp = RateLimitResponse::new();
        resp.overall_code = RateLimitResponse_Code::OK;
        resp.response_headers_to_add
            .push(header("test", "some value"));
        resp.response_headers_to_add
            .push(header("other", "header value"));
        let buffer = resp
            .write_to_bytes()
            .expect("must be able to write RateLimitResponse to bytes");
        let expected: [u8; 45] = [
            8, 1, 26, 18, 10, 4, 116, 101, 115, 116, 18, 10, 115, 111, 109, 101, 32, 118, 97, 108,
            117, 101, 26, 21, 10, 5, 111, 116, 104, 101, 114, 18, 12, 104, 101, 97, 100, 101, 114,
            32, 118, 97, 108, 117, 101,
        ];
        assert_eq!(expected, buffer.as_slice())
    }

    fn header(key: &str, value: &str) -> HeaderValue {
        let mut header = HeaderValue::new();
        header.key = key.to_string();
        header.value = value.to_string();
        header
    }
}
