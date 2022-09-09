use crate::envoy::{HeaderMatcher, HeaderMatcher_specifier, StringMatcher_pattern};
use proxy_wasm::hostcalls::{resume_http_request, send_http_response};
use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum UtilsErr {
    #[error("failed to create string from utf8 data")]
    Utf8Fail(#[from] std::string::FromUtf8Error),
    #[error("problem while handing protobuf")]
    ProtobufErr(#[from] protobuf::error::ProtobufError),
    #[error("failed to get i64 from slice")]
    SliceToI64(#[from] std::array::TryFromSliceError),
    #[error("failed to convert from i64 to u32")]
    I64ToU32(#[from] std::num::TryFromIntError),
}

// Helper function to handle failure during processing.
pub fn request_process_failure(failure_mode_deny: bool) {
    if failure_mode_deny {
        send_http_response(500, vec![], Some(b"Internal Server Error.\n")).unwrap();
    }
    resume_http_request().unwrap();
}

pub fn match_headers(
    req_headers: &HashMap<String, String>,
    config_headers: &[HeaderMatcher],
) -> bool {
    for header_matcher in config_headers {
        let invert_match = header_matcher.get_invert_match();
        if let Some(req_header_value) = req_headers.get(header_matcher.get_name()) {
            if let Some(hm_specifier) = &header_matcher.header_match_specifier {
                let mut is_match = false;
                match hm_specifier {
                    HeaderMatcher_specifier::exact_match(str) => is_match = str == req_header_value,
                    HeaderMatcher_specifier::safe_regex_match(_regex_matcher) => todo!(), // TODO(rahulanand16nov): not implemented.
                    HeaderMatcher_specifier::range_match(range) => {
                        if let Ok(val) = req_header_value.parse::<i64>() {
                            is_match = range.get_start() <= val && val < range.get_end();
                        }
                    }
                    HeaderMatcher_specifier::present_match(should_be_present) => {
                        is_match = *should_be_present
                    }
                    HeaderMatcher_specifier::prefix_match(prefix) => {
                        is_match = req_header_value.starts_with(prefix.as_str())
                    }
                    HeaderMatcher_specifier::suffix_match(suffix) => {
                        is_match = req_header_value.ends_with(suffix.as_str())
                    }
                    HeaderMatcher_specifier::contains_match(str) => {
                        is_match = req_header_value.contains(str.as_str())
                    }
                    HeaderMatcher_specifier::string_match(str_matcher) => {
                        let ignore_case = str_matcher.get_ignore_case();
                        if let Some(pattern) = &str_matcher.match_pattern {
                            match pattern {
                                StringMatcher_pattern::exact(str) => {
                                    is_match = if ignore_case {
                                        str.eq_ignore_ascii_case(req_header_value)
                                    } else {
                                        str == req_header_value
                                    }
                                }
                                StringMatcher_pattern::prefix(str) => {
                                    is_match = if ignore_case {
                                        req_header_value
                                            .to_lowercase()
                                            .starts_with(&str.to_lowercase())
                                    } else {
                                        req_header_value.starts_with(str.as_str())
                                    }
                                }
                                StringMatcher_pattern::suffix(str) => {
                                    is_match = if ignore_case {
                                        req_header_value
                                            .to_lowercase()
                                            .ends_with(&str.to_lowercase())
                                    } else {
                                        req_header_value.ends_with(str.as_str())
                                    }
                                }
                                StringMatcher_pattern::safe_regex(_) => todo!(), // TODO(rahulanand16nov): not implemented.
                                StringMatcher_pattern::contains(str) => {
                                    is_match = if ignore_case {
                                        req_header_value
                                            .to_lowercase()
                                            .contains(&str.to_lowercase())
                                    } else {
                                        req_header_value.contains(str.as_str())
                                    }
                                }
                            }
                        } else {
                            return false;
                        }
                    }
                }
                if is_match ^ invert_match {
                    return false;
                }
            } else {
                return false;
            }
        } else {
            return false;
        }
    }
    true
}

pub fn subdomain_match(subdomain: &str, authority: &str) -> bool {
    authority.ends_with(subdomain.replace('*', "").as_str())
}

pub fn path_match(path_pattern: &str, request_path: &str) -> bool {
    if path_pattern.ends_with('*') {
        let mut cp = path_pattern.to_string();
        cp.pop();
        request_path.starts_with(cp.as_str())
    } else {
        request_path.eq(path_pattern)
    }
}

#[cfg(test)]
mod tests {
    use crate::utils;

    #[test]
    fn subdomain_match() {
        assert!(utils::subdomain_match("*.example.com", "test.example.com"));
        assert!(!utils::subdomain_match("*.example.com", "example.com"));
        assert!(utils::subdomain_match("*", "test1.test2.example.com"));
        assert!(utils::subdomain_match("example.com", "example.com"));
    }

    #[test]
    fn path_match() {
        assert!(utils::path_match("/cats", "/cats"));
        assert!(utils::path_match("/", "/"));
        assert!(utils::path_match("/*", "/"));
        assert!(utils::path_match("/*", "/cats/something"));
        assert!(utils::path_match("/cats/*", "/cats/"));
        assert!(utils::path_match("/cats/*", "/cats/something"));
        assert!(!utils::path_match("/cats/*", "/cats"));
        assert!(utils::path_match("/cats*", "/catsanddogs"));
        assert!(utils::path_match("/cats*", "/cats/dogs"));
    }
}
