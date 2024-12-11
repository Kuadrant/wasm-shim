use crate::auth_action::AuthAction;
use crate::configuration::{Action, FailureMode, Service, ServiceType};
use crate::filter::proposal_context::no_implicit_dep::PendingOperation;
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

    pub fn create_message(&self) -> crate::service::GrpcRequest {
        self.grpc_service().build_request(None)
    }

    pub fn process(&self) -> Option<PendingOperation> {
        if !self.conditions_apply() {
            None
        } else {
            // if provided message return what?
            // if no message we assume we're a sender??????????
            todo!()
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::configuration::{Action, FailureMode, ServiceType, Timeout};

    pub enum Operation {
        SendRequest(RequestSender),
        ConsumeRequest(RequestConsumer),
    }
    type RequestSender = ();
    // struct RequestSender {}
    // impl RequestSender {
    //
    // }

    type RequestConsumer = ();
    // struct RequestConsumer {}
    // impl RequestConsumer {
    //
    // }

    #[test]
    fn start_action_set_flow() {
        let actions = vec![
            RuntimeAction::new(&build_action("ratelimit", "scope"), &HashMap::default()).unwrap(),
            RuntimeAction::new(&build_action("ratelimit", "scope"), &HashMap::default()).unwrap(),
        ];
        let mut iter = actions.iter();
        let a = iter.next().expect("get the first action");

        // let op: Result<Option<RequestSender>, ()> = a.create_message(); // action.?
        // let ret: RequestSender = match op {
        //     Ok(_) => unreachable!("should have failed"),
        //     Err(_) => match iter.next() {
        //         Some(b) => b.create_message().expect("Ok").expect("Some"),
        //         None => (),
        //     },
        // };

        // this is caller code

        // on_http_request:find_action_set:start_flow

        // let (message_handler, req) = ret.create_request();
        // let token = send_request(req);

        // let (message, handler) = ret.create_request() // SendMessageOperation -> ReceiveMessageOperation
        // how does this function look?
        // does it take into account current action?

        // on_grpc_response

        // let response = message_handler.consume(response);

        /* bs
        let next = action_set.progress(op);
        action_set.actions[op.action_index].progress(op);

        struct Operation {
            current: RuntimeAction,
            next: Option<Operation>
        }
        */
    }

    /* Overall
    - We have Operation that transitions between different states passing the ref to ActionSet
      to subsequent actions as well as an index
    - The action_set has either a start_flow function, or maybe just process?
        + this iterates over the actions to find the next one
    - The runtime_action has the ability to create a message?

    */

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
