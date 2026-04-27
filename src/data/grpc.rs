/// Returns `true` if the content-type indicates a gRPC request.
pub(crate) fn is_grpc_content_type(content_type: &str) -> bool {
    content_type.starts_with("application/grpc")
}

/// Parses a gRPC path of the form `/Service/Method`.
/// Returns `None` for paths that don't match the expected format.
pub(crate) fn parse_grpc_path(path: &str) -> Option<(String, String)> {
    let trimmed = path.strip_prefix('/')?;
    let (service, method) = trimmed.split_once('/')?;
    if service.is_empty() || method.is_empty() || method.contains('/') {
        return None;
    }
    Some((service.to_string(), method.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_grpc_content_type() {
        let tests = vec![
            ("application/grpc", true),
            ("application/grpc+proto", true),
            ("application/grpc+json", true),
            ("application/json", false),
            ("text/plain", false),
            ("", false),
        ];

        for (input, expected) in tests {
            assert_eq!(
                is_grpc_content_type(input),
                expected,
                "is_grpc_content_type({input:?})"
            );
        }
    }

    #[test]
    fn test_parse_grpc_path() {
        let tests: Vec<(&str, Option<(&str, &str)>)> = vec![
            ("/UserService/GetUser", Some(("UserService", "GetUser"))),
            (
                "/com.example.UserService/GetUser",
                Some(("com.example.UserService", "GetUser")),
            ),
            ("/Service/Method/Extra", None),
            ("/Service/", None),
            ("//Method", None),
            ("Service/Method", None),
            ("/", None),
            ("", None),
        ];

        for (input, expected) in tests {
            let result = parse_grpc_path(input);
            let expected = expected.map(|(s, m)| (s.to_string(), m.to_string()));
            assert_eq!(result, expected, "parse_grpc_path({input:?})");
        }
    }
}
