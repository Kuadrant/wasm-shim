use crate::auth_action::AuthAction;
use crate::configuration::{Action, FailureMode, Service, ServiceType};
use crate::data::{
    Attribute, AttributeOwner, AttributeResolver, Expression, Predicate, PredicateResult,
};
use crate::filter::operations::{
    EventualOperation, ProcessGrpcMessageOperation, ProcessNextRequestOperation,
};
use crate::ratelimit_action::RateLimitAction;
use crate::runtime_action::errors::ActionCreationError;
use crate::service::auth::AuthService;
use crate::service::errors::{BuildMessageError, ProcessGrpcMessageError};
use crate::service::GrpcService;
use crate::v2::temp::GrpcRequest;
use log::debug;
use protobuf::Message;
use std::collections::HashMap;
use std::rc::Rc;

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
                    write!(f, "NewActionError::Parse {{ {e:?} }}")
                }
                ActionCreationError::UnknownService(e) => {
                    write!(f, "NewActionError::UnknownService {{ {e:?} }}")
                }
            }
        }
    }
}

pub type NextRequestResult = Result<ProcessNextRequestOperation, BuildMessageError>;
pub type RequestResult = Result<Option<GrpcRequest>, BuildMessageError>;
pub type ResponseResult = Result<ProcessGrpcMessageOperation, ProcessGrpcMessageError>;

impl RuntimeAction {
    pub fn new(
        action: &Action,
        services: &HashMap<String, Service>,
        request_data: Vec<((String, String), Expression)>,
    ) -> Result<Self, ActionCreationError> {
        let service = services
            .get(&action.service)
            .ok_or(ActionCreationError::UnknownService(format!(
                "Unknown service: {}",
                action.service
            )))?;

        match service.service_type {
            ServiceType::RateLimit | ServiceType::RateLimitCheck | ServiceType::RateLimitReport => {
                Ok(Self::RateLimit(RateLimitAction::new_with_data(
                    action,
                    service,
                    request_data,
                )?))
            }
            ServiceType::Auth => Ok(Self::Auth(AuthAction::new(action, service, request_data)?)),
        }
    }

    pub fn grpc_service(&self) -> Rc<GrpcService> {
        match self {
            Self::Auth(auth_action) => auth_action.get_grpcservice(),
            Self::RateLimit(rl_action) => rl_action.get_grpcservice(),
        }
    }

    fn conditions_apply<T>(&self, resolver: &mut T) -> PredicateResult
    where
        T: AttributeResolver,
    {
        match self {
            Self::Auth(auth_action) => auth_action.conditions_apply(resolver),
            Self::RateLimit(rl_action) => rl_action.conditions_apply(),
        }
    }

    pub fn get_failure_mode(&self) -> FailureMode {
        match self {
            Self::Auth(auth_action) => auth_action.get_failure_mode(),
            Self::RateLimit(rl_action) => rl_action.get_failure_mode(),
        }
    }

    pub fn build_request<T>(&self, resolver: &mut T) -> RequestResult
    where
        T: AttributeResolver,
    {
        match self.conditions_apply(resolver) {
            Ok(false) => Ok(None),
            Ok(true) => {
                let message = self.build_message(resolver)?;
                Ok(self.grpc_service().build_request(message))
            }
            Err(e) => Err(e.into()),
        }
    }

    pub fn process_response(&self, msg: &[u8]) -> ResponseResult {
        let res = match self {
            Self::Auth(auth_action) => {
                let check_response = Message::parse_from_bytes(msg)?;
                auth_action.process_response(check_response)
            }
            Self::RateLimit(rl_action) => {
                let rate_limit_response = Message::parse_from_bytes(msg)?;
                rl_action.process_response(rate_limit_response)
            }
        };

        match res {
            Ok(operation) => Ok(operation),
            Err(ProcessGrpcMessageError::UnsupportedField) => {
                // this case should error (direct response / stop flow) regardless of FailureMode
                // the fields are unsupported by the external auth service
                // and could be an indication of a man in the middle attack,
                // so the request should not proceed
                Err(ProcessGrpcMessageError::UnsupportedField)
            }
            Err(e) => match self.get_failure_mode() {
                FailureMode::Deny => Err(e),
                FailureMode::Allow => {
                    debug!("continuing as FailureMode Allow");
                    let ops: Vec<EventualOperation> = vec![];
                    Ok(ops.into())
                }
            },
        }
    }

    fn build_message<T>(&self, resolver: &mut T) -> Result<Option<Vec<u8>>, BuildMessageError>
    where
        T: AttributeResolver,
    {
        match self {
            RuntimeAction::RateLimit(rl_action) => rl_action.build_message(resolver),
            RuntimeAction::Auth(auth_action) => AuthService::request_message_as_bytes(
                String::from(auth_action.scope()),
                auth_action.request_data(),
                resolver,
            )
            .map(Some),
        }
    }
}

impl AttributeOwner for RuntimeAction {
    fn request_attributes(&self) -> Vec<&Attribute> {
        match self {
            Self::Auth(auth_action) => auth_action.request_attributes(),
            Self::RateLimit(rl_action) => rl_action.request_attributes(),
        }
    }
}

impl AttributeOwner for Vec<Predicate> {
    fn request_attributes(&self) -> Vec<&Attribute> {
        self.iter()
            .flat_map(|predicate| predicate.request_attributes())
            .collect()
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

    fn build_action(service: &str, scope: &str) -> Action {
        Action {
            service: service.into(),
            scope: scope.into(),
            predicates: Vec::default(),
            conditional_data: Vec::default(),
        }
    }

    #[test]
    fn action_request_attribute() {
        let mut services = HashMap::new();
        services.insert(String::from("service_rl"), build_rl_service());

        let mut action = build_action("service_rl", "scope");
        let conditional_data = vec![
            DataItem {
                item: DataType::Expression(ExpressionItem {
                    key: "key_1".into(),
                    value: "request.host".into(),
                }),
            },
            DataItem {
                item: DataType::Expression(ExpressionItem {
                    key: "key_2".into(),
                    value: "request.method".into(),
                }),
            },
            // duplicated attribute
            DataItem {
                item: DataType::Expression(ExpressionItem {
                    key: "key_3".into(),
                    value: "request.method".into(),
                }),
            },
        ];
        action
            .conditional_data
            .extend(conditional_data.into_iter().map(|data_item| {
                crate::configuration::ConditionalData {
                    predicates: Vec::default(),
                    data: vec![data_item],
                }
            }));

        let runtime_action = RuntimeAction::new(&action, &services, Vec::default()).unwrap();

        assert_eq!(runtime_action.request_attributes().len(), 3);
    }
}
