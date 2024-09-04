use crate::envoy::RateLimitDescriptor;
use crate::policy::Policy;
use crate::service::{GrpcMessage, GrpcServiceHandler};
use protobuf::RepeatedField;
use proxy_wasm::types::Status;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[allow(dead_code)]
#[derive(PartialEq, Debug, Clone)]
pub(crate) enum State {
    Pending,
    Waiting,
    Done,
}

#[allow(dead_code)]
impl State {
    fn next(&mut self) {
        match self {
            State::Pending => *self = State::Waiting,
            State::Waiting => *self = State::Done,
            _ => {}
        }
    }
}

type Procedure = (Rc<GrpcServiceHandler>, GrpcMessage);

#[allow(dead_code)]
pub(crate) struct Operation {
    state: State,
    result: Result<u32, Status>,
    procedure: Procedure,
}

#[allow(dead_code)]
impl Operation {
    pub fn new(procedure: Procedure) -> Self {
        Self {
            state: State::Pending,
            result: Err(Status::Empty),
            procedure,
        }
    }

    pub fn set_action(&mut self, procedure: Procedure) {
        self.procedure = procedure;
    }

    pub fn trigger(&mut self) {
        if let State::Done = self.state {
        } else {
            self.result = self.procedure.0.send(self.procedure.1.clone());
            self.state.next();
        }
    }

    fn get_state(&self) -> State {
        self.state.clone()
    }

    fn get_result(&self) -> Result<u32, Status> {
        self.result
    }
}

#[allow(dead_code)]
pub struct OperationDispatcher {
    operations: RefCell<Vec<Operation>>,
    service_handlers: HashMap<String, Rc<GrpcServiceHandler>>,
}

#[allow(dead_code)]
impl OperationDispatcher {
    pub fn default() -> Self {
        OperationDispatcher {
            operations: RefCell::new(vec![]),
            service_handlers: HashMap::default(),
        }
    }
    pub fn new(service_handlers: HashMap<String, Rc<GrpcServiceHandler>>) -> Self {
        Self {
            service_handlers,
            operations: RefCell::new(vec![]),
        }
    }

    pub fn build_operations(
        &self,
        policy: &Policy,
        descriptors: RepeatedField<RateLimitDescriptor>,
    ) {
        let mut operations: Vec<Operation> = vec![];
        policy.actions.iter().for_each(|action| {
            // TODO(didierofrivia): Error handling
            if let Some(service) = self.service_handlers.get(&action.extension) {
                let message = GrpcMessage::new(
                    service.get_extension_type(),
                    policy.domain.clone(),
                    descriptors.clone(),
                );
                operations.push(Operation::new((service.clone(), message)))
            }
        });
        self.push_operations(operations);
    }

    pub fn push_operations(&self, operations: Vec<Operation>) {
        self.operations.borrow_mut().extend(operations);
    }

    pub fn get_current_operation_state(&self) -> Option<State> {
        self.operations
            .borrow()
            .first()
            .map(|operation| operation.get_state().clone())
    }

    pub fn get_current_operation_result(&self) -> Result<u32, Status> {
        self.operations.borrow().first().unwrap().get_result()
    }

    pub fn next(&self) -> Option<(State, Result<u32, Status>)> {
        let mut operations = self.operations.borrow_mut();
        if let Some((i, operation)) = operations.iter_mut().enumerate().next() {
            if let State::Done = operation.get_state() {
                let res = operation.get_result();
                operations.remove(i);
                Some((State::Done, res))
            } else {
                operation.trigger();
                Some((operation.state.clone(), operation.result))
            }
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envoy::RateLimitRequest;
    use std::time::Duration;

    fn grpc_call(
        _upstream_name: &str,
        _service_name: &str,
        _method_name: &str,
        _initial_metadata: Vec<(&str, &[u8])>,
        _message: Option<&[u8]>,
        _timeout: Duration,
    ) -> Result<u32, Status> {
        Ok(1)
    }

    fn build_grpc_service_handler() -> GrpcServiceHandler {
        GrpcServiceHandler::new(
            Rc::new(Default::default()),
            Rc::new(Default::default()),
            Some(grpc_call),
        )
    }

    fn build_message() -> RateLimitRequest {
        RateLimitRequest {
            domain: "example.org".to_string(),
            descriptors: RepeatedField::new(),
            hits_addend: 1,
            unknown_fields: Default::default(),
            cached_size: Default::default(),
        }
    }

    #[test]
    fn operation_transition() {
        let mut operation = Operation::new((
            Rc::new(build_grpc_service_handler()),
            GrpcMessage::RateLimit(build_message()),
        ));
        assert_eq!(operation.get_state(), State::Pending);
        operation.trigger();
        assert_eq!(operation.get_state(), State::Waiting);
        operation.trigger();
        assert_eq!(operation.result, Ok(200));
        assert_eq!(operation.get_state(), State::Done);
    }

    #[test]
    fn operation_dispatcher_push_actions() {
        let operation_dispatcher = OperationDispatcher::default();

        assert_eq!(operation_dispatcher.operations.borrow().len(), 1);

        operation_dispatcher.push_operations(vec![Operation::new((
            Rc::new(build_grpc_service_handler()),
            GrpcMessage::RateLimit(build_message()),
        ))]);

        assert_eq!(operation_dispatcher.operations.borrow().len(), 2);
    }

    #[test]
    fn operation_dispatcher_get_current_action_state() {
        let operation_dispatcher = OperationDispatcher::default();

        assert_eq!(
            operation_dispatcher.get_current_operation_state(),
            Some(State::Pending)
        );
    }

    #[test]
    fn operation_dispatcher_next() {
        let operation = Operation::new((
            Rc::new(build_grpc_service_handler()),
            GrpcMessage::RateLimit(build_message()),
        ));
        let operation_dispatcher = OperationDispatcher::default();
        operation_dispatcher.push_operations(vec![operation]);

        let mut res = operation_dispatcher.next();
        assert_eq!(res, Some((State::Waiting, Ok(200))));
        assert_eq!(
            operation_dispatcher.get_current_operation_state(),
            Some(State::Waiting)
        );

        res = operation_dispatcher.next();
        assert_eq!(res, Some((State::Done, Ok(200))));
        assert_eq!(
            operation_dispatcher.get_current_operation_state(),
            Some(State::Done)
        );
        assert_eq!(operation_dispatcher.get_current_operation_result(), Ok(200));

        res = operation_dispatcher.next();
        assert_eq!(res, None);
        assert_eq!(operation_dispatcher.get_current_operation_state(), None);
    }
}
