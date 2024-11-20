use crate::configuration::{Action, FailureMode, Service};
use crate::data::Predicate;
use crate::service::GrpcService;
use log::error;
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
        let predicates = &self.predicates;
        predicates.is_empty()
            || predicates.iter().all(|predicate| match predicate.test() {
                Ok(b) => b,
                Err(err) => {
                    error!("Failed to evaluate {:?}: {}", predicate, err);
                    panic!("Err out of this!")
                }
            })
    }

    pub fn get_failure_mode(&self) -> FailureMode {
        self.grpc_service.get_failure_mode()
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
