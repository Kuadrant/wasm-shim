use crate::auth_action::AuthAction;
use crate::configuration::{Action, FailureMode, Service, ServiceType};
use crate::data::{Expression, Predicate, PredicateResult};
use crate::ratelimit_action::RateLimitAction;
use crate::runtime_action::errors::ActionCreationError;
use crate::service::auth::AuthService;
use crate::service::errors::BuildMessageError;
use crate::service::rate_limit::RateLimitService;
use crate::service::{GrpcErrResponse, GrpcRequest, GrpcService, HeaderKind};
use log::debug;
use protobuf::Message;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    OnRequestHeaders,
    OnRequestBody,
}

#[derive(Debug, PartialEq, Clone)]
pub enum RuntimeAction {
    Auth(AuthAction),
    RateLimit(RateLimitAction),
}

pub(super) mod errors {
    use cel_parser::ParseError;
    use std::fmt::{Debug, Display, Formatter};

    #[derive(Debug)]
    pub enum ActionCreationError {
        Parse(ParseError),
        UnknownService(String),
        InvalidAction(String),
    }

    impl From<ParseError> for ActionCreationError {
        fn from(e: ParseError) -> ActionCreationError {
            ActionCreationError::Parse(e)
        }
    }

    impl PartialEq for ActionCreationError {
        fn eq(&self, other: &ActionCreationError) -> bool {
            match (self, other) {
                (ActionCreationError::Parse(_), ActionCreationError::Parse(_)) => false,
                (
                    ActionCreationError::UnknownService(a),
                    ActionCreationError::UnknownService(b),
                ) => a == b,
                _ => false,
            }
        }
    }

    impl Display for ActionCreationError {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            match self {
                ActionCreationError::Parse(e) => {
                    write!(f, "NewActionError::Parse {{ {:?} }}", e)
                }
                ActionCreationError::UnknownService(e) => {
                    write!(f, "NewActionError::UnknownService {{ {:?} }}", e)
                }
                ActionCreationError::InvalidAction(e) => {
                    write!(f, "NewActionError::InvalidAction {{ {:?} }}", e)
                }
            }
        }
    }
}

pub type RequestResult = Result<Option<GrpcRequest>, GrpcErrResponse>;
pub type ResponseResult = Result<HeaderKind, GrpcErrResponse>;

impl RuntimeAction {
    pub fn new(
        action: &Action,
        services: &HashMap<String, Service>,
    ) -> Result<Self, ActionCreationError> {
        let service = services
            .get(&action.service)
            .ok_or(ActionCreationError::UnknownService(format!(
                "Unknown service: {}",
                action.service
            )))?;

        match service.service_type {
            ServiceType::RateLimit => Ok(Self::RateLimit(RateLimitAction::new(action, service)?)),
            ServiceType::Auth => Ok(Self::Auth(AuthAction::new(action, service)?)),
        }
    }

    pub fn grpc_service(&self) -> Rc<GrpcService> {
        match self {
            Self::Auth(auth_action) => auth_action.get_grpcservice(),
            Self::RateLimit(rl_action) => rl_action.get_grpcservice(),
        }
    }

    pub fn conditions_apply(&self) -> PredicateResult {
        match self {
            Self::Auth(auth_action) => auth_action.conditions_apply(),
            Self::RateLimit(rl_action) => rl_action.conditions_apply(),
        }
    }

    pub fn get_failure_mode(&self) -> FailureMode {
        match self {
            Self::Auth(auth_action) => auth_action.get_failure_mode(),
            Self::RateLimit(rl_action) => rl_action.get_failure_mode(),
        }
    }

    pub fn resolve_failure_mode(&self) -> ResponseResult {
        match self {
            Self::Auth(auth_action) => auth_action.resolve_failure_mode(),
            Self::RateLimit(rl_action) => rl_action.resolve_failure_mode(),
        }
    }

    pub fn merge(
        &mut self,
        other: RuntimeAction,
    ) -> Result<Option<RuntimeAction>, ActionCreationError> {
        // only makes sense for rate limiting actions
        match (self, other) {
            (Self::RateLimit(self_rl_action), Self::RateLimit(other_rl_action)) => {
                match self_rl_action.merge(other_rl_action) {
                    Ok(None) => Ok(None),
                    Ok(Some(unmerged_action)) => Ok(Some(Self::RateLimit(unmerged_action))),
                    Err(e) => Err(e),
                }
            }
            (_, unmatched_other) => Ok(Some(unmatched_other)),
        }
    }

    pub fn process_request(&self) -> RequestResult {
        match self.conditions_apply() {
            Ok(false) => Ok(None),
            Ok(true) => match self.build_message() {
                Ok(message) => Ok(self.grpc_service().build_request(message)),
                Err(_) => self.resolve_failure_mode().map(|_| None),
            },
            Err(_) => self.resolve_failure_mode().map(|_| None),
        }
    }

    pub fn process_response(&self, msg: &[u8]) -> ResponseResult {
        match self {
            Self::Auth(auth_action) => match Message::parse_from_bytes(msg) {
                Ok(check_response) => auth_action.process_response(check_response),
                Err(e) => {
                    debug!("process_response(auth): failed to parse response `{e:?}`");
                    self.resolve_failure_mode()
                }
            },
            Self::RateLimit(rl_action) => match Message::parse_from_bytes(msg) {
                Ok(rate_limit_response) => rl_action.process_response(rate_limit_response),
                Err(e) => {
                    debug!("process_response(rl): failed to parse response `{e:?}`");
                    self.resolve_failure_mode()
                }
            },
        }
    }

    pub fn build_message(&self) -> Result<Option<Vec<u8>>, BuildMessageError> {
        match self {
            RuntimeAction::RateLimit(rl_action) => {
                let descriptor = rl_action.build_descriptor()?;
                let (hits_addend, domain_attr) = rl_action.get_known_attributes()?;

                if descriptor.entries.is_empty() {
                    debug!("build_message(rl): empty descriptors");
                    Ok(None)
                } else {
                    let domain = if domain_attr.is_empty() {
                        rl_action.scope().to_string()
                    } else {
                        domain_attr
                    };

                    RateLimitService::request_message_as_bytes(
                        domain,
                        vec![descriptor].into(),
                        hits_addend,
                    )
                    .map(Some)
                }
            }
            RuntimeAction::Auth(auth_action) => {
                AuthService::request_message_as_bytes(String::from(auth_action.scope())).map(Some)
            }
        }
    }
}

fn minimum_required_phase(fn_names: Vec<&str>) -> Phase {
    match fn_names.iter().any(|&x| x == "requestBodyJSON") {
        true => Phase::OnRequestBody,
        false => Phase::OnRequestHeaders,
    }
}

pub trait MinimumRequiredPhase {
    fn phase(&self) -> Phase;
}

impl MinimumRequiredPhase for RuntimeAction {
    fn phase(&self) -> Phase {
        match self {
            Self::Auth(auth_action) => auth_action.phase(),
            Self::RateLimit(rl_action) => rl_action.phase(),
        }
    }
}

impl MinimumRequiredPhase for Predicate {
    fn phase(&self) -> Phase {
        minimum_required_phase(self.fn_names())
    }
}

impl MinimumRequiredPhase for Expression {
    fn phase(&self) -> Phase {
        minimum_required_phase(self.fn_names())
    }
}

impl MinimumRequiredPhase for Vec<Predicate> {
    fn phase(&self) -> Phase {
        match self
            .iter()
            .any(|predicate| predicate.phase() == Phase::OnRequestBody)
        {
            true => Phase::OnRequestBody,
            false => Phase::OnRequestHeaders,
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::configuration::{
        Action, DataItem, DataType, ExpressionItem, FailureMode, ServiceType, Timeout,
    };
    fn build_rl_service() -> Service {
        Service {
            service_type: ServiceType::RateLimit,
            endpoint: "limitador".into(),
            failure_mode: FailureMode::default(),
            timeout: Timeout::default(),
        }
    }

    fn build_auth_service() -> Service {
        Service {
            service_type: ServiceType::Auth,
            endpoint: "authorino".into(),
            failure_mode: FailureMode::default(),
            timeout: Timeout::default(),
        }
    }

    fn build_action(service: &str, scope: &str) -> Action {
        Action {
            service: service.into(),
            scope: scope.into(),
            predicates: Vec::default(),
            data: Vec::default(),
        }
    }

    #[test]
    fn only_rl_actions_are_merged() {
        let mut services = HashMap::new();
        services.insert(String::from("service_rl"), build_rl_service());

        let rl_action_0 = build_action("service_rl", "scope");
        let rl_action_1 = build_action("service_rl", "scope");

        let mut rl_r_action_0 = RuntimeAction::new(&rl_action_0, &services)
            .expect("action building failed. Maybe predicates compilation?");
        let rl_r_action_1 = RuntimeAction::new(&rl_action_1, &services)
            .expect("action building failed. Maybe predicates compilation?");

        let result = rl_r_action_0.merge(rl_r_action_1.clone());
        assert_eq!(result, Ok(None));
    }

    #[test]
    fn auth_actions_are_not_merged() {
        let mut services = HashMap::new();
        services.insert(String::from("service_auth"), build_auth_service());

        let auth_action_0 = build_action("service_auth", "scope");
        let auth_action_1 = build_action("service_auth", "scope");

        let mut auth_r_action_0 = RuntimeAction::new(&auth_action_0, &services)
            .expect("action building failed. Maybe predicates compilation?");
        let auth_r_action_1 = RuntimeAction::new(&auth_action_1, &services)
            .expect("action building failed. Maybe predicates compilation?");

        let result = auth_r_action_0.merge(auth_r_action_1.clone());
        assert_eq!(result, Ok(Some(auth_r_action_1)));
    }

    #[test]
    fn auth_actions_do_not_merge_rl() {
        let mut services = HashMap::new();
        services.insert(String::from("service_rl"), build_rl_service());
        services.insert(String::from("service_auth"), build_auth_service());

        let rl_action_0 = build_action("service_rl", "scope");
        let auth_action_0 = build_action("service_auth", "scope");

        let mut rl_r_action_0 = RuntimeAction::new(&rl_action_0, &services)
            .expect("action building failed. Maybe predicates compilation?");

        let auth_r_action_0 = RuntimeAction::new(&auth_action_0, &services)
            .expect("action building failed. Maybe predicates compilation?");

        let result = rl_r_action_0.merge(auth_r_action_0.clone());
        assert_eq!(result, Ok(Some(auth_r_action_0)));
    }

    #[test]
    fn rl_actions_do_not_merge_auth() {
        let mut services = HashMap::new();
        services.insert(String::from("service_rl"), build_rl_service());
        services.insert(String::from("service_auth"), build_auth_service());

        let rl_action_0 = build_action("service_rl", "scope");
        let auth_action_0 = build_action("service_auth", "scope");

        let rl_r_action_0 = RuntimeAction::new(&rl_action_0, &services)
            .expect("action building failed. Maybe predicates compilation?");

        let mut auth_r_action_0 = RuntimeAction::new(&auth_action_0, &services)
            .expect("action building failed. Maybe predicates compilation?");

        let result = auth_r_action_0.merge(rl_r_action_0.clone());
        assert_eq!(result, Ok(Some(rl_r_action_0)));
    }

    #[test]
    fn test_build_message_uses_known_attributes() {
        let mut services = HashMap::new();
        services.insert(String::from("service_rl"), build_rl_service());

        let mut action = build_action("service_rl", "scope");
        let data = vec![
            DataItem {
                item: DataType::Expression(ExpressionItem {
                    key: "key_1".into(),
                    value: "'value_1'".into(),
                }),
            },
            DataItem {
                item: DataType::Expression(ExpressionItem {
                    key: "ratelimit.domain".into(),
                    value: "'test'".into(),
                }),
            },
            DataItem {
                item: DataType::Expression(ExpressionItem {
                    key: "ratelimit.hits_addend".into(),
                    value: "1+1".into(),
                }),
            },
        ];
        action.data.extend(data);

        let runtime_action = RuntimeAction::new(&action, &services).unwrap();

        if let RuntimeAction::RateLimit(ref rl_action) = runtime_action {
            let (hits_addend, domain) = rl_action.get_known_attributes().unwrap();
            assert_eq!(hits_addend, 2);
            assert_eq!(domain, "test");
        }
    }

    #[test]
    fn minimum_required_phase() {
        // empty inputnames
        assert_eq!(
            super::minimum_required_phase(Vec::default()),
            Phase::OnRequestHeaders
        );

        // requestBodyJSON not in function names
        let fn_names = vec!["func_a", "func_b"];
        assert_eq!(
            super::minimum_required_phase(fn_names),
            Phase::OnRequestHeaders
        );

        // requestBodyJSON in function names
        let fn_names = vec!["func_a", "func_b", "requestBodyJSON", "func_c"];
        assert_eq!(
            super::minimum_required_phase(fn_names),
            Phase::OnRequestBody
        );
    }

    #[test]
    fn expression_minimum_required_phase() {
        // requestBodyJSON not in expression
        let expr =
            Expression::new("func_a([func_b(), 'bar.baz', 'looks_like_a_func(56)', 4.func_c()])")
                .expect("This is valid CEL!");
        assert_eq!(expr.phase(), Phase::OnRequestHeaders);

        // requestBodyJSON not as expression function but exists as part of the expression str
        let expr =
            Expression::new("func_a([func_b(), 'bar.baz', 'requestBodyJSON(56)', 4.func_c()])")
                .expect("This is valid CEL!");
        assert_eq!(expr.phase(), Phase::OnRequestHeaders);

        // requestBodyJSON in expression
        let expr =
            Expression::new("func_a([func_b(), 'bar.baz', requestBodyJSON(56), 4.func_c()])")
                .expect("This is valid CEL!");
        assert_eq!(expr.phase(), Phase::OnRequestBody);
    }

    #[test]
    fn predicate_minimum_required_phase() {
        // requestBodyJSON not in predicate
        let predicate =
            Predicate::new("func_a([func_b(), 'bar.baz', 'looks_like_a_func(56)', 4.func_c()])")
                .expect("This is valid CEL!");
        assert_eq!(predicate.phase(), Phase::OnRequestHeaders);

        // requestBodyJSON not as expression function but exists as part of the expression str
        let predicate =
            Predicate::new("func_a([func_b(), 'bar.baz', 'requestBodyJSON(56)', 4.func_c()])")
                .expect("This is valid CEL!");
        assert_eq!(predicate.phase(), Phase::OnRequestHeaders);

        // requestBodyJSON in expression
        let predicate =
            Predicate::new("func_a([func_b(), 'bar.baz', requestBodyJSON(56), 4.func_c()])")
                .expect("This is valid CEL!");
        assert_eq!(predicate.phase(), Phase::OnRequestBody);
    }

    #[test]
    fn vec_predicate_minimum_required_phase() {
        // empty
        let predicates: Vec<Predicate> = Vec::default();
        assert_eq!(predicates.phase(), Phase::OnRequestHeaders);

        // all on_request_headers
        let predicates: Vec<Predicate> = vec![
            Predicate::new("1 + 1 == 2").expect("this is a valid CEL!"),
            Predicate::new("1 + 2 == 3").expect("this is a valid CEL!"),
        ];
        assert_eq!(predicates.phase(), Phase::OnRequestHeaders);

        // one on_request_body after on_request_headers
        let predicates: Vec<Predicate> = vec![
            Predicate::new("1 + 1 == 2").expect("this is a valid CEL!"),
            Predicate::new("requestBodyJSON('model')").expect("this is a valid CEL!"),
        ];
        assert_eq!(predicates.phase(), Phase::OnRequestBody);

        // one on_request_headers after on_request_body
        let predicates: Vec<Predicate> = vec![
            Predicate::new("requestBodyJSON('model')").expect("this is a valid CEL!"),
            Predicate::new("1 + 1 == 2").expect("this is a valid CEL!"),
        ];
        assert_eq!(predicates.phase(), Phase::OnRequestBody);
    }
}
