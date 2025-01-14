use crate::auth_action::AuthAction;
use crate::configuration::{Action, FailureMode, Service, ServiceType};
use crate::envoy::{CheckResponse, RateLimitResponse};
use crate::ratelimit_action::RateLimitAction;
use crate::service::auth::AuthService;
use crate::service::rate_limit::RateLimitService;
use crate::service::{GrpcErrResponse, GrpcRequest, GrpcService};
use log::debug;
use protobuf::Message;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Debug)]
pub enum RuntimeAction {
    Auth(AuthAction),
    RateLimit(RateLimitAction),
}

impl RuntimeAction {
    pub fn new(action: &Action, services: &HashMap<String, Service>) -> Result<Self, String> {
        let service = services
            .get(&action.service)
            .ok_or(format!("Unknown service: {}", action.service))?;

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

    pub fn conditions_apply(&self) -> bool {
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

    #[must_use]
    pub fn merge(&mut self, other: RuntimeAction) -> Option<RuntimeAction> {
        // only makes sense for rate limiting actions
        if let Self::RateLimit(self_rl_action) = self {
            if let Self::RateLimit(other_rl_action) = other {
                return self_rl_action.merge(other_rl_action).map(Self::RateLimit);
            }
        }
        Some(other)
    }

    pub fn process_request(&self) -> Option<GrpcRequest> {
        if !self.conditions_apply() {
            None
        } else {
            self.grpc_service().build_request(self.build_message())
        }
    }

    pub fn process_response(
        &self,
        msg: &[u8],
    ) -> Result<Option<Vec<(String, String)>>, GrpcErrResponse> {
        match self {
            Self::Auth(auth_action) => {
                // todo(adam-cattermole): should this expect be here?
                let check_response: CheckResponse =
                    Message::parse_from_bytes(msg).expect("invalid state!");
                auth_action.process_response(check_response)
            }
            Self::RateLimit(rl_action) => {
                let rate_limit_response: RateLimitResponse =
                    Message::parse_from_bytes(msg).expect("invalid state!");
                rl_action.process_response(rate_limit_response)
            }
        }
    }

    pub fn build_message(&self) -> Option<Vec<u8>> {
        match self {
            RuntimeAction::RateLimit(rl_action) => {
                let descriptor = rl_action.build_descriptor();
                if descriptor.entries.is_empty() {
                    debug!("build_message(rl): empty descriptors");
                    None
                } else {
                    RateLimitService::request_message_as_bytes(
                        String::from(rl_action.scope()),
                        vec![descriptor].into(),
                    )
                }
            }
            RuntimeAction::Auth(auth_action) => {
                AuthService::request_message_as_bytes(String::from(auth_action.scope()))
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::configuration::{Action, FailureMode, ServiceType, Timeout};

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

        assert!(rl_r_action_0.merge(rl_r_action_1).is_none());
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

        assert!(auth_r_action_0.merge(auth_r_action_1).is_some());
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

        assert!(rl_r_action_0.merge(auth_r_action_0).is_some());
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

        assert!(auth_r_action_0.merge(rl_r_action_0).is_some());
    }
}
