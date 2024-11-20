use crate::auth_action::AuthAction;
use crate::configuration::{Action, FailureMode, Service, ServiceType};
use crate::ratelimit_action::RateLimitAction;
use crate::service::GrpcService;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

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

    pub fn get_timeout(&self) -> Duration {
        self.grpc_service().get_timeout()
    }

    pub fn get_service_type(&self) -> ServiceType {
        self.grpc_service().get_service_type()
    }
}
