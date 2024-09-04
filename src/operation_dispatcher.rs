use crate::configuration::{Extension, ExtensionType, FailureMode};
use crate::envoy::RateLimitDescriptor;
use crate::policy::Policy;
use crate::service::{GetMapValuesBytes, GrpcCall, GrpcMessage, GrpcServiceHandler};
use protobuf::RepeatedField;
use proxy_wasm::hostcalls;
use proxy_wasm::types::{Bytes, MapType, Status};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

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
#[derive(Clone)]
pub(crate) struct Operation {
    state: State,
    result: Result<u32, Status>,
    extension: Rc<Extension>,
    procedure: Procedure,
    grpc_call: GrpcCall,
    get_map_values_bytes: GetMapValuesBytes,
}

#[allow(dead_code)]
impl Operation {
    pub fn new(extension: Rc<Extension>, procedure: Procedure) -> Self {
        Self {
            state: State::Pending,
            result: Err(Status::Empty),
            extension,
            procedure,
            grpc_call,
            get_map_values_bytes,
        }
    }

    fn trigger(&mut self) {
        if let State::Done = self.state {
        } else {
            self.result = self.procedure.0.send(
                self.get_map_values_bytes,
                self.grpc_call,
                self.procedure.1.clone(),
            );
            self.state.next();
        }
    }

    pub fn get_state(&self) -> State {
        self.state.clone()
    }

    pub fn get_result(&self) -> Result<u32, Status> {
        self.result
    }

    pub fn get_extension_type(&self) -> ExtensionType {
        self.extension.extension_type.clone()
    }

    pub fn get_failure_mode(&self) -> FailureMode {
        self.extension.failure_mode.clone()
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
                operations.push(Operation::new(
                    service.get_extension(),
                    (Rc::clone(service), message),
                ))
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

    pub fn next(&self) -> Option<Operation> {
        let mut operations = self.operations.borrow_mut();
        if let Some((i, operation)) = operations.iter_mut().enumerate().next() {
            if let State::Done = operation.get_state() {
                Some(operations.remove(i))
            } else {
                operation.trigger();
                Some(operation.clone())
            }
        } else {
            None
        }
    }
}

fn grpc_call(
    upstream_name: &str,
    service_name: &str,
    method_name: &str,
    initial_metadata: Vec<(&str, &[u8])>,
    message: Option<&[u8]>,
    timeout: Duration,
) -> Result<u32, Status> {
    hostcalls::dispatch_grpc_call(
        upstream_name,
        service_name,
        method_name,
        initial_metadata,
        message,
        timeout,
    )
}

fn get_map_values_bytes(map_type: MapType, key: &str) -> Result<Option<Bytes>, Status> {
    hostcalls::get_map_value_bytes(map_type, key)
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
        Ok(200)
    }

    fn get_map_values_bytes(_map_type: MapType, _key: &str) -> Result<Option<Bytes>, Status> {
        Ok(Some(Vec::new()))
    }

    fn build_grpc_service_handler() -> GrpcServiceHandler {
        GrpcServiceHandler::new(Rc::new(Default::default()), Rc::new(Default::default()))
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

    fn build_operation() -> Operation {
        Operation {
            state: State::Pending,
            result: Ok(200),
            extension: Rc::new(Extension::default()),
            procedure: (
                Rc::new(build_grpc_service_handler()),
                GrpcMessage::RateLimit(build_message()),
            ),
            grpc_call,
            get_map_values_bytes,
        }
    }

    #[test]
    fn operation_getters() {
        let operation = build_operation();

        assert_eq!(operation.get_state(), State::Pending);
        assert_eq!(operation.get_extension_type(), ExtensionType::RateLimit);
        assert_eq!(operation.get_failure_mode(), FailureMode::Deny);
        assert_eq!(operation.get_result(), Ok(200));
    }

    #[test]
    fn operation_transition() {
        let mut operation = build_operation();
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

        assert_eq!(operation_dispatcher.operations.borrow().len(), 0);
        operation_dispatcher.push_operations(vec![build_operation()]);

        assert_eq!(operation_dispatcher.operations.borrow().len(), 1);
    }

    #[test]
    fn operation_dispatcher_get_current_action_state() {
        let operation_dispatcher = OperationDispatcher::default();
        operation_dispatcher.push_operations(vec![build_operation()]);
        assert_eq!(
            operation_dispatcher.get_current_operation_state(),
            Some(State::Pending)
        );
    }

    #[test]
    fn operation_dispatcher_next() {
        let operation = build_operation();
        let operation_dispatcher = OperationDispatcher::default();
        operation_dispatcher.push_operations(vec![operation]);

        if let Some(operation) = operation_dispatcher.next() {
            assert_eq!(operation.get_result(), Ok(200));
            assert_eq!(operation.get_state(), State::Waiting);
        }

        if let Some(operation) = operation_dispatcher.next() {
            assert_eq!(operation.get_result(), Ok(200));
            assert_eq!(operation.get_state(), State::Done);
        }
        operation_dispatcher.next();
        assert_eq!(operation_dispatcher.get_current_operation_state(), None);
    }
}
