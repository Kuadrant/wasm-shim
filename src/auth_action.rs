use crate::configuration::{Action, FailureMode, Service};
use crate::data::{store_metadata, Predicate, PredicateVec};
use crate::envoy::{CheckResponse, CheckResponse_oneof_http_response, HeaderValueOption};
use crate::service::{GrpcErrResponse, GrpcService};
use log::debug;
use std::rc::Rc;

#[derive(Debug)]
pub struct AuthAction {
    grpc_service: Rc<GrpcService>,
    scope: String,
    predicates: Vec<Predicate>,
}

impl AuthAction {
    pub fn new(action: &Action, service: &Service) -> Result<Self, String> {
        let mut predicates = Vec::default();
        for predicate in &action.predicates {
            predicates.push(Predicate::new(predicate).map_err(|e| e.to_string())?);
        }

        Ok(AuthAction {
            grpc_service: Rc::new(GrpcService::new(Rc::new(service.clone()))),
            scope: action.scope.clone(),
            predicates,
        })
    }

    pub fn get_grpcservice(&self) -> Rc<GrpcService> {
        Rc::clone(&self.grpc_service)
    }

    pub fn scope(&self) -> &str {
        self.scope.as_str()
    }

    pub fn conditions_apply(&self) -> bool {
        self.predicates.apply()
    }

    pub fn get_failure_mode(&self) -> FailureMode {
        self.grpc_service.get_failure_mode()
    }

    pub fn process_response(
        &self,
        check_response: CheckResponse,
    ) -> Result<Option<Vec<(String, String)>>, GrpcErrResponse> {
        //todo(adam-cattermole):hostvar resolver?
        // store dynamic metadata in filter state
        debug!("process_response(auth): store_metadata");
        store_metadata(check_response.get_dynamic_metadata());

        match check_response.http_response {
            None => {
                debug!("process_response(auth): received no http_response");
                match self.get_failure_mode() {
                    FailureMode::Deny => Err(GrpcErrResponse::new_internal_server_error()),
                    FailureMode::Allow => {
                        debug!("process_response(auth): continuing as FailureMode Allow");
                        Ok(None)
                    }
                }
            }
            Some(CheckResponse_oneof_http_response::denied_response(denied_response)) => {
                debug!("process_response(auth): received DeniedHttpResponse");
                let status_code = denied_response.get_status().get_code();
                let response_headers = Self::get_header_vec(denied_response.get_headers());
                Err(GrpcErrResponse::new(
                    status_code as u32,
                    response_headers,
                    denied_response.body,
                ))
            }
            Some(CheckResponse_oneof_http_response::ok_response(ok_response)) => {
                debug!("process_response(auth): received OkHttpResponse");

                if !ok_response.get_response_headers_to_add().is_empty() {
                    panic!("process_response(auth): response contained response_headers_to_add which is unsupported!")
                }
                if !ok_response.get_headers_to_remove().is_empty() {
                    panic!("process_response(auth): response contained headers_to_remove which is unsupported!")
                }
                if !ok_response.get_query_parameters_to_set().is_empty() {
                    panic!("process_response(auth): response contained query_parameters_to_set which is unsupported!")
                }
                if !ok_response.get_query_parameters_to_remove().is_empty() {
                    panic!("process_response(auth): response contained query_parameters_to_remove which is unsupported!")
                }

                let response_headers = Self::get_header_vec(ok_response.get_headers());
                if response_headers.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(response_headers))
                }
            }
        }
    }

    fn get_header_vec(headers: &[HeaderValueOption]) -> Vec<(String, String)> {
        headers
            .iter()
            .map(|header| {
                let hv = header.get_header();
                (hv.key.to_owned(), hv.value.to_owned())
            })
            .collect()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::configuration::{Action, FailureMode, Service, ServiceType, Timeout};
    use crate::envoy::{DeniedHttpResponse, HeaderValue, HttpStatus, OkHttpResponse, StatusCode};
    use protobuf::RepeatedField;

    fn build_auth_action_with_predicates(predicates: Vec<String>) -> AuthAction {
        build_auth_action_with_predicates_and_failure_mode(predicates, FailureMode::default())
    }

    fn build_auth_action_with_predicates_and_failure_mode(
        predicates: Vec<String>,
        failure_mode: FailureMode,
    ) -> AuthAction {
        let action = Action {
            service: "some_service".into(),
            scope: "some_scope".into(),
            predicates,
            data: Vec::default(),
        };

        let service = Service {
            service_type: ServiceType::Auth,
            endpoint: "some_endpoint".into(),
            failure_mode,
            timeout: Timeout::default(),
        };

        AuthAction::new(&action, &service)
            .expect("action building failed. Maybe predicates compilation?")
    }

    fn build_check_response(
        status: StatusCode,
        headers: Option<Vec<(&str, &str)>>,
        body: Option<String>,
    ) -> CheckResponse {
        let mut response = CheckResponse::new();
        match status {
            StatusCode::OK => {
                let mut ok_http_response = OkHttpResponse::new();
                if let Some(header_list) = headers {
                    ok_http_response.set_headers(build_headers(header_list))
                }
                response.set_ok_response(ok_http_response);
            }
            StatusCode::Forbidden => {
                let mut http_status = HttpStatus::new();
                http_status.set_code(status);

                let mut denied_http_response = DeniedHttpResponse::new();
                denied_http_response.set_status(http_status);
                if let Some(header_list) = headers {
                    denied_http_response.set_headers(build_headers(header_list));
                }
                denied_http_response.set_body(body.unwrap_or_default());
                response.set_denied_response(denied_http_response);
            }
            _ => {
                // assume any other code is for error state
            }
        };
        response
    }

    fn build_headers(headers: Vec<(&str, &str)>) -> RepeatedField<HeaderValueOption> {
        headers
            .into_iter()
            .map(|(key, value)| {
                let header_value = {
                    let mut hv = HeaderValue::new();
                    hv.set_key(key.to_string());
                    hv.set_value(value.to_string());
                    hv
                };
                let mut header_option = HeaderValueOption::new();
                header_option.set_header(header_value);
                header_option
            })
            .collect::<RepeatedField<HeaderValueOption>>()
    }

    #[test]
    fn empty_predicates_do_apply() {
        let auth_action = build_auth_action_with_predicates(Vec::default());
        assert!(auth_action.conditions_apply());
    }

    #[test]
    fn when_all_predicates_are_truthy_action_apply() {
        let auth_action = build_auth_action_with_predicates(vec!["true".into(), "true".into()]);
        assert!(auth_action.conditions_apply());
    }

    #[test]
    fn when_not_all_predicates_are_truthy_action_does_not_apply() {
        let auth_action = build_auth_action_with_predicates(vec![
            "true".into(),
            "true".into(),
            "true".into(),
            "false".into(),
        ]);
        assert!(!auth_action.conditions_apply());
    }

    #[test]
    #[should_panic]
    fn when_a_cel_expression_does_not_evaluate_to_bool_panics() {
        let auth_action = build_auth_action_with_predicates(vec![
            "true".into(),
            "true".into(),
            "true".into(),
            "1".into(),
        ]);
        auth_action.conditions_apply();
    }

    #[test]
    fn process_ok_response() {
        let auth_action = build_auth_action_with_predicates(Vec::default());
        let ok_response_without_headers = build_check_response(StatusCode::OK, None, None);
        let result = auth_action.process_response(ok_response_without_headers);
        assert!(result.is_ok());

        let headers = result.expect("is ok");
        assert!(headers.is_none());

        let ok_response_with_header =
            build_check_response(StatusCode::OK, Some(vec![("my_header", "my_value")]), None);
        let result = auth_action.process_response(ok_response_with_header);
        assert!(result.is_ok());

        let headers = result.expect("is ok");
        assert!(headers.is_some());

        let header_vec = headers.expect("is some");
        assert_eq!(
            header_vec[0],
            ("my_header".to_string(), "my_value".to_string())
        );
    }

    #[test]
    fn process_denied_response() {
        let headers = vec![
            ("www-authenticate", "APIKEY realm=\"api-key-users\""),
            ("x-ext-auth-reason", "credential not found"),
        ];
        let auth_action = build_auth_action_with_predicates(Vec::default());
        let denied_response_empty = build_check_response(StatusCode::Forbidden, None, None);
        let result = auth_action.process_response(denied_response_empty);
        assert!(result.is_err());

        let grpc_err_response = result.expect_err("is err");
        assert_eq!(
            grpc_err_response.status_code(),
            StatusCode::Forbidden as u32
        );
        assert!(grpc_err_response.headers().is_empty());
        assert_eq!(grpc_err_response.body(), String::default());

        let denied_response_content = build_check_response(
            StatusCode::Forbidden,
            Some(headers.clone()),
            Some("my_body".to_string()),
        );
        let result = auth_action.process_response(denied_response_content);
        assert!(result.is_err());

        let grpc_err_response = result.expect_err("is err");
        assert_eq!(
            grpc_err_response.status_code(),
            StatusCode::Forbidden as u32
        );

        let response_headers = grpc_err_response.headers();
        headers.iter().zip(response_headers.iter()).for_each(
            |((header_one, value_one), (header_two, value_two))| {
                assert_eq!(header_one, header_two);
                assert_eq!(value_one, value_two);
            },
        );

        assert_eq!(grpc_err_response.body(), "my_body");
    }

    #[test]
    fn process_error_response() {
        let auth_action =
            build_auth_action_with_predicates_and_failure_mode(Vec::default(), FailureMode::Deny);
        let error_response = build_check_response(StatusCode::InternalServerError, None, None);
        let result = auth_action.process_response(error_response);
        assert!(result.is_err());

        let grpc_err_response = result.expect_err("is err");
        assert_eq!(
            grpc_err_response.status_code(),
            StatusCode::InternalServerError as u32
        );

        assert!(grpc_err_response.headers().is_empty());
        assert_eq!(grpc_err_response.body(), "Internal Server Error.\n");

        let auth_action =
            build_auth_action_with_predicates_and_failure_mode(Vec::default(), FailureMode::Allow);
        let error_response = build_check_response(StatusCode::InternalServerError, None, None);
        let result = auth_action.process_response(error_response);
        assert!(result.is_ok());

        let headers = result.expect("is ok");
        assert!(headers.is_none());
    }
}
