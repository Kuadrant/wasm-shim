use crate::configuration::{Action, FailureMode, Service};
use crate::data::{store_metadata, Predicate, PredicateVec};
use crate::envoy::{CheckResponse, CheckResponse_oneof_http_response, HeaderValueOption};
use crate::filter::proposal_context::no_implicit_dep::{
    EndRequestOperation, HeadersOperation, Operation,
};
use crate::service::GrpcService;
use log::debug;
use protobuf::Message;
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

    pub fn process_response(&self, check_response: CheckResponse) -> Operation {
        //todo(adam-cattermole):error handling, ...
        debug!("process_response: auth");

        // store dynamic metadata in filter state
        debug!("process_response: store_metadata");
        store_metadata(check_response.get_dynamic_metadata());

        match check_response.http_response {
            Some(CheckResponse_oneof_http_response::ok_response(ok_response)) => {
                debug!("process_auth_grpc_response: received OkHttpResponse");
                if !ok_response.get_response_headers_to_add().is_empty() {
                    panic!("process_auth_grpc_response: response contained response_headers_to_add which is unsupported!")
                }
                if !ok_response.get_headers_to_remove().is_empty() {
                    panic!("process_auth_grpc_response: response contained headers_to_remove which is unsupported!")
                }
                if !ok_response.get_query_parameters_to_set().is_empty() {
                    panic!("process_auth_grpc_response: response contained query_parameters_to_set which is unsupported!")
                }
                if !ok_response.get_query_parameters_to_remove().is_empty() {
                    panic!("process_auth_grpc_response: response contained query_parameters_to_remove which is unsupported!")
                }

                let response_headers = Self::get_header_vec(ok_response.get_headers());
                if !response_headers.is_empty() {
                    Operation::AddHeaders(HeadersOperation::new(response_headers))
                } else {
                    Operation::Done()
                }
            }
            Some(CheckResponse_oneof_http_response::denied_response(denied_response)) => {
                debug!("process_auth_grpc_response: received DeniedHttpResponse");
                let status_code = denied_response.get_status().code;
                let response_headers = Self::get_header_vec(denied_response.get_headers());
                Operation::Die(EndRequestOperation::new(
                    status_code as u32,
                    response_headers,
                    Some(denied_response.body),
                ))
            }
            None => Operation::Die(EndRequestOperation::default()),
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

    fn build_auth_action_with_predicates(predicates: Vec<String>) -> AuthAction {
        let action = Action {
            service: "some_service".into(),
            scope: "some_scope".into(),
            predicates,
            data: Vec::default(),
        };

        let service = Service {
            service_type: ServiceType::Auth,
            endpoint: "some_endpoint".into(),
            failure_mode: FailureMode::default(),
            timeout: Timeout::default(),
        };

        AuthAction::new(&action, &service)
            .expect("action building failed. Maybe predicates compilation?")
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
}
