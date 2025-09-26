use crate::configuration::{Action, FailureMode, Service};
use crate::data::{
    store_metadata, Attribute, AttributeOwner, AttributeResolver, Expression, Predicate,
    PredicateResult, PredicateVec,
};
use crate::envoy::check_response::HttpResponse;
use crate::envoy::CheckResponse;
use crate::filter::operations::EventualOperation;
use crate::runtime_action::ResponseResult;
use crate::service::errors::ProcessGrpcMessageError;
use crate::service::{from_envoy_headers, DirectResponse, GrpcService};
use cel_parser::ParseError;
use log::{debug, warn};
use std::rc::Rc;

#[derive(Debug, PartialEq, Clone)]
pub struct AuthAction {
    grpc_service: Rc<GrpcService>,
    scope: String,
    predicates: Vec<Predicate>,
    request_data: Vec<((String, String), Expression)>,
}

impl AuthAction {
    pub fn new(
        action: &Action,
        service: &Service,
        request_data: Vec<((String, String), Expression)>,
    ) -> Result<Self, ParseError> {
        let mut predicates = Vec::default();
        for predicate in &action.predicates {
            predicates.push(Predicate::new(predicate)?);
        }

        Ok(AuthAction {
            grpc_service: Rc::new(GrpcService::new(Rc::new(service.clone()))),
            scope: action.scope.clone(),
            predicates,
            request_data,
        })
    }

    pub fn get_grpcservice(&self) -> Rc<GrpcService> {
        Rc::clone(&self.grpc_service)
    }

    pub fn scope(&self) -> &str {
        self.scope.as_str()
    }

    pub fn request_data(&self) -> &Vec<((String, String), Expression)> {
        &self.request_data
    }

    pub fn conditions_apply<T>(&self, resolver: &mut T) -> PredicateResult
    where
        T: AttributeResolver,
    {
        self.predicates.apply(resolver)
    }

    pub fn get_failure_mode(&self) -> FailureMode {
        self.grpc_service.get_failure_mode()
    }

    pub fn process_response(&self, check_response: CheckResponse) -> ResponseResult {
        //todo(adam-cattermole):hostvar resolver?
        // store dynamic metadata in filter state
        debug!("process_response(auth): store_metadata");
        if let Some(metadata) = &check_response.dynamic_metadata {
            store_metadata(metadata)
        } else {
            Ok(())
        }
        .map_err(ProcessGrpcMessageError::Property)?;

        match check_response.http_response {
            None => {
                debug!("process_response(auth): received no http_response");
                Err(ProcessGrpcMessageError::EmptyResponse)
            }
            Some(HttpResponse::DeniedResponse(denied_response)) => {
                debug!("process_response(auth): received DeniedHttpResponse");
                let direct_response: DirectResponse = denied_response.into();
                Ok(direct_response.into())
            }
            Some(HttpResponse::OkResponse(ok_response)) => {
                debug!("process_response(auth): received OkHttpResponse");

                if !ok_response.response_headers_to_add.is_empty() {
                    warn!("process_response(auth): Unsupported field 'response_headers_to_add' in OkHttpResponse");
                    Err(ProcessGrpcMessageError::UnsupportedField)
                } else if !ok_response.headers_to_remove.is_empty() {
                    warn!("process_response(auth): Unsupported field 'headers_to_remove' in OkHttpResponse");
                    Err(ProcessGrpcMessageError::UnsupportedField)
                } else if !ok_response.query_parameters_to_set.is_empty() {
                    warn!("process_response(auth): Unsupported field 'query_parameters_to_set' in OkHttpResponse");
                    Err(ProcessGrpcMessageError::UnsupportedField)
                } else if !ok_response.query_parameters_to_remove.is_empty() {
                    warn!("process_response(auth): Unsupported field 'query_parameters_to_remove' in OkHttpResponse");
                    Err(ProcessGrpcMessageError::UnsupportedField)
                } else {
                    Ok(
                        vec![EventualOperation::AddRequestHeaders(from_envoy_headers(
                            &ok_response.headers,
                        ))]
                        .into(),
                    )
                }
            }
        }
    }
}

impl AttributeOwner for AuthAction {
    fn request_attributes(&self) -> Vec<&Attribute> {
        let request_data_attrs = self
            .request_data
            .iter()
            .flat_map(|((_, _), exp)| exp.request_attributes());
        request_data_attrs
            .chain(self.predicates.request_attributes())
            .collect()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::configuration::{Action, FailureMode, Service, ServiceType, Timeout};
    use crate::data::PathCache;
    use crate::envoy::{
        DeniedHttpResponse, HeaderValueOption, HttpStatus, OkHttpResponse, StatusCode,
    };
    use crate::filter::operations::ProcessGrpcMessageOperation;

    fn build_auth_action_with_predicates(predicates: Vec<String>) -> AuthAction {
        build_auth_action_with_predicates_and_failure_mode(
            predicates,
            FailureMode::default(),
            Vec::default(),
        )
    }

    fn build_auth_action_with_predicates_and_failure_mode(
        predicates: Vec<String>,
        failure_mode: FailureMode,
        request_data: Vec<((String, String), Expression)>,
    ) -> AuthAction {
        let action = Action {
            service: "some_service".into(),
            scope: "some_scope".into(),
            predicates,
            conditional_data: Vec::default(),
        };

        let service = Service {
            service_type: ServiceType::Auth,
            endpoint: "some_endpoint".into(),
            failure_mode,
            timeout: Timeout::default(),
        };

        AuthAction::new(&action, &service, request_data)
            .expect("action building failed. Maybe predicates compilation?")
    }

    fn build_check_response(
        status: StatusCode,
        headers: Option<Vec<(&str, &str)>>,
        body: Option<String>,
    ) -> CheckResponse {
        let http_response = match status {
            StatusCode::Ok => {
                let headers = headers.map(build_headers).unwrap_or_default();
                Some(HttpResponse::OkResponse(OkHttpResponse {
                    headers,
                    ..Default::default()
                }))
            }
            StatusCode::Forbidden => {
                let headers = headers.map(build_headers).unwrap_or_default();
                Some(HttpResponse::DeniedResponse(DeniedHttpResponse {
                    status: Some(HttpStatus {
                        code: status as i32,
                    }),
                    headers,
                    body: body.unwrap_or_default(),
                }))
            }
            _ => None,
        };
        CheckResponse {
            status: None,
            dynamic_metadata: None,
            http_response,
        }
    }

    fn build_headers(headers: Vec<(&str, &str)>) -> Vec<HeaderValueOption> {
        headers
            .into_iter()
            .map(|(key, value)| HeaderValueOption {
                header: Some(crate::envoy::envoy::config::core::v3::HeaderValue {
                    key: key.to_string(),
                    value: value.to_string(),
                }),
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn empty_predicates_do_apply() {
        let mut resolver = PathCache::default();
        let auth_action = build_auth_action_with_predicates(Vec::default());
        let res = auth_action
            .conditions_apply(&mut resolver)
            .expect("this is a valid predicate!");
        assert!(res);
    }

    #[test]
    fn when_all_predicates_are_truthy_action_apply() {
        let mut resolver = PathCache::default();
        let auth_action = build_auth_action_with_predicates(vec!["true".into(), "true".into()]);
        let res = auth_action
            .conditions_apply(&mut resolver)
            .expect("this is a valid predicate!");
        assert!(res);
    }

    #[test]
    fn when_not_all_predicates_are_truthy_action_does_not_apply() {
        let mut resolver = PathCache::default();
        let auth_action = build_auth_action_with_predicates(vec![
            "true".into(),
            "true".into(),
            "true".into(),
            "false".into(),
        ]);
        let res = auth_action
            .conditions_apply(&mut resolver)
            .expect("this is a valid predicate!");
        assert!(!res);
    }

    #[test]
    fn when_a_cel_expression_does_not_evaluate_to_bool_panics() {
        let mut resolver = PathCache::default();
        let auth_action = build_auth_action_with_predicates(vec![
            "true".into(),
            "true".into(),
            "true".into(),
            "1".into(),
        ]);
        assert!(auth_action.conditions_apply(&mut resolver).is_err());
    }

    #[test]
    fn process_ok_response() {
        let auth_action = build_auth_action_with_predicates(Vec::default());
        let ok_response_without_headers = build_check_response(StatusCode::Ok, None, None);
        let result = auth_action.process_response(ok_response_without_headers);
        assert!(result.is_ok());
        let op = result.expect("is ok");
        assert!(matches!(op, ProcessGrpcMessageOperation::EventualOps(_)));
        if let ProcessGrpcMessageOperation::EventualOps(ops) = op {
            assert_eq!(ops.len(), 1);
            assert!(matches!(ops[0], EventualOperation::AddRequestHeaders(_)));
            if let EventualOperation::AddRequestHeaders(headers) = &ops[0] {
                assert!(headers.is_empty());
            } else {
                unreachable!("Expected AddRequestHeaders operation");
            }
        } else {
            unreachable!("Expected EventualOps operation");
        }

        let ok_response_with_header =
            build_check_response(StatusCode::Ok, Some(vec![("my_header", "my_value")]), None);
        let result = auth_action.process_response(ok_response_with_header);
        assert!(result.is_ok());
        let op = result.expect("is ok");
        assert!(matches!(op, ProcessGrpcMessageOperation::EventualOps(_)));
        if let ProcessGrpcMessageOperation::EventualOps(ops) = op {
            assert_eq!(ops.len(), 1);
            assert!(matches!(ops[0], EventualOperation::AddRequestHeaders(_)));
            if let EventualOperation::AddRequestHeaders(headers) = &ops[0] {
                assert_eq!(headers.len(), 1);
                assert_eq!(
                    headers[0],
                    ("my_header".to_string(), "my_value".to_string())
                );
            } else {
                unreachable!("Expected AddRequestHeaders operation");
            }
        } else {
            unreachable!("Expected EventualOps operation");
        }
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
        assert!(result.is_ok());
        let op = result.expect("is ok");
        assert!(matches!(op, ProcessGrpcMessageOperation::DirectResponse(_)));

        let denied_response_content = build_check_response(
            StatusCode::Forbidden,
            Some(headers.clone()),
            Some("my_body".to_string()),
        );
        let result = auth_action.process_response(denied_response_content);
        assert!(result.is_ok());

        let op = result.expect("is ok");
        assert!(matches!(op, ProcessGrpcMessageOperation::DirectResponse(_)));
        if let ProcessGrpcMessageOperation::DirectResponse(direct_response) = op {
            assert_eq!(
                direct_response.status_code(),
                crate::envoy::envoy::r#type::v3::StatusCode::Forbidden as u32
            );
            let response_headers = direct_response.headers();
            headers.iter().zip(response_headers.iter()).for_each(
                |((header_one, value_one), (header_two, value_two))| {
                    assert_eq!(header_one, header_two);
                    assert_eq!(value_one, value_two);
                },
            );
            assert_eq!(direct_response.body(), "my_body");
        } else {
            unreachable!("Expected DirectResponse operation");
        }
    }

    #[test]
    fn process_error_response() {
        let auth_action = build_auth_action_with_predicates_and_failure_mode(
            Vec::default(),
            FailureMode::Deny,
            Vec::default(),
        );
        let error_response = build_check_response(StatusCode::InternalServerError, None, None);
        let result = auth_action.process_response(error_response);
        assert!(result.is_err());

        let err_response = result.expect_err("is err");
        assert!(matches!(
            err_response,
            ProcessGrpcMessageError::EmptyResponse
        ));
    }

    #[test]
    fn auth_action_request_attributes() {
        let predicates: Vec<String> = vec!["true".into(), "request.method == 'GET'".into()];
        let request_data: Vec<((String, String), Expression)> = vec![
            (
                ("metrics.labels".into(), "foo".into()),
                Expression::new("request.path").expect("should compile"),
            ),
            (
                ("metrics.labels".into(), "bar".into()),
                Expression::new("source.port").expect("should compile"),
            ),
        ];
        let action = build_auth_action_with_predicates_and_failure_mode(
            predicates,
            FailureMode::Deny,
            request_data,
        );

        assert_eq!(action.request_attributes().len(), 2);
    }
}
