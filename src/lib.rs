mod configuration;
mod envoy;
mod filter;
mod glob;
mod policy_index;
mod utils;

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
        let buffer = resp.write_to_bytes().unwrap();
        // assert_eq!(b"", buffer.as_slice())
    }

    fn header(key: &str, value: &str) -> HeaderValue {
        let mut header = HeaderValue::new();
        header.key = key.to_string();
        header.value = value.to_string();
        header
    }
}
