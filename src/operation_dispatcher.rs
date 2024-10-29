use crate::configuration::action::Action;
use crate::configuration::{FailureMode, Service, ServiceType};
use crate::service::grpc_message::GrpcMessageRequest;
use crate::service::{GetMapValuesBytesFn, GrpcCallFn, GrpcMessageBuildFn, GrpcServiceHandler};
use log::{debug, error};
use proxy_wasm::hostcalls;
use proxy_wasm::types::{Bytes, MapType, Status};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;
use std::time::Duration;

#[derive(PartialEq, Debug, Clone, Copy)]
pub(crate) enum State {
    Pending,
    Waiting,
    Done,
}

impl State {
    fn next(&mut self) {
        match self {
            State::Pending => *self = State::Waiting,
            State::Waiting => *self = State::Done,
            _ => {}
        }
    }

    fn done(&mut self) {
        *self = State::Done
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Operation {
    state: RefCell<State>,
    result: RefCell<Result<u32, OperationError>>,
    service: Rc<Service>,
    action: Action,
    service_handler: Rc<GrpcServiceHandler>,
    grpc_call_fn: GrpcCallFn,
    get_map_values_bytes_fn: GetMapValuesBytesFn,
    grpc_message_build_fn: GrpcMessageBuildFn,
    conditions_apply_fn: ConditionsApplyFn,
}

impl Operation {
    pub fn new(
        service: Rc<Service>,
        action: Action,
        service_handler: Rc<GrpcServiceHandler>,
    ) -> Self {
        Self {
            state: RefCell::new(State::Pending),
            result: RefCell::new(Ok(0)), // Heuristics: zero represents that it's not been triggered, following `hostcalls` example
            service,
            action,
            service_handler,
            grpc_call_fn,
            get_map_values_bytes_fn,
            grpc_message_build_fn,
            conditions_apply_fn,
        }
    }

    fn trigger(&self) -> Result<u32, OperationError> {
        if let Some(message) = (self.grpc_message_build_fn)(self.get_service_type(), &self.action) {
            let res = self.service_handler.send(
                self.get_map_values_bytes_fn,
                self.grpc_call_fn,
                message,
                self.service.timeout.0,
            );
            match res {
                Ok(token_id) => self.set_result(Ok(token_id)),
                Err(status) => {
                    self.set_result(Err(OperationError::new(status, self.get_failure_mode())))
                }
            }
            self.next_state();
            self.get_result()
        } else {
            self.done();
            self.get_result()
        }
    }

    fn next_state(&self) {
        self.state.borrow_mut().next()
    }

    fn done(&self) {
        self.state.borrow_mut().done()
    }

    pub fn get_state(&self) -> State {
        *self.state.borrow()
    }

    pub fn get_result(&self) -> Result<u32, OperationError> {
        *self.result.borrow()
    }

    fn set_result(&self, result: Result<u32, OperationError>) {
        *self.result.borrow_mut() = result;
    }

    pub fn get_service_type(&self) -> &ServiceType {
        &self.service.service_type
    }

    pub fn get_failure_mode(&self) -> FailureMode {
        self.service.failure_mode
    }
}
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct OperationError {
    pub status: Status,
    pub failure_mode: FailureMode,
}

impl OperationError {
    fn new(status: Status, failure_mode: FailureMode) -> Self {
        Self {
            status,
            failure_mode,
        }
    }
}

impl fmt::Display for OperationError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.status {
            Status::Empty => {
                write!(f, "No more operations to perform.")
            }
            Status::ParseFailure => {
                write!(f, "Error parsing configuration.")
            }
            _ => {
                write!(f, "Error triggering the operation. {:?}", self.status)
            }
        }
    }
}

pub struct OperationDispatcher {
    operations: Vec<Rc<Operation>>,
    waiting_operations: HashMap<u32, Rc<Operation>>,
    service_handlers: HashMap<String, Rc<GrpcServiceHandler>>,
}

impl OperationDispatcher {
    pub fn new(service_handlers: HashMap<String, Rc<GrpcServiceHandler>>) -> Self {
        Self {
            service_handlers,
            operations: vec![],
            waiting_operations: HashMap::new(),
        }
    }

    pub fn get_waiting_operation(&self, token_id: u32) -> Result<Rc<Operation>, OperationError> {
        let op = self.waiting_operations.get(&token_id);
        match op {
            Some(op) => {
                op.next_state();
                Ok(op.clone())
            }
            None => Err(OperationError::new(
                Status::NotFound,
                FailureMode::default(),
            )),
        }
    }

    pub fn build_operations(&mut self, actions: &[Action]) -> Result<(), OperationError> {
        let mut operations: Vec<Rc<Operation>> = vec![];
        for action in actions.iter() {
            if let Some(service) = self.service_handlers.get(&action.service) {
                operations.push(Rc::new(Operation::new(
                    service.get_service(),
                    action.clone(),
                    Rc::clone(service),
                )))
            } else {
                error!("Unknown service: {}", action.service);
                return Err(OperationError::new(
                    Status::ParseFailure,
                    Default::default(),
                ));
            }
        }
        self.push_operations(operations);
        Ok(())
    }

    pub fn push_operations(&mut self, operations: Vec<Rc<Operation>>) {
        self.operations.extend(operations);
    }

    pub fn next(&mut self) -> Result<Rc<Operation>, OperationError> {
        if let Some((i, operation)) = self.operations.iter_mut().enumerate().next() {
            match operation.get_state() {
                State::Pending => {
                    if (operation.conditions_apply_fn)(&operation.action) {
                        match operation.trigger() {
                            Ok(token_id) => {
                                match operation.get_state() {
                                    State::Pending => {
                                        panic!("Operation dispatcher reached an undefined state");
                                    }
                                    State::Waiting => {
                                        // We index only if it was just transitioned to Waiting after triggering
                                        self.waiting_operations.insert(token_id, operation.clone());
                                        // TODO(didierofrivia): Decide on indexing the failed operations.
                                        Ok(operation.clone())
                                    }
                                    State::Done => self.next(),
                                }
                            }
                            Err(err) => {
                                error!("{err:?}");
                                Err(err)
                            }
                        }
                    } else {
                        debug!("actions conditions do not apply, skipping");
                        self.operations.remove(i);
                        self.next()
                    }
                }
                State::Waiting => {
                    operation.next_state();
                    Ok(operation.clone())
                }
                State::Done => {
                    if let Ok(token_id) = operation.get_result() {
                        self.waiting_operations.remove(&token_id);
                    } // If result was Err, means the operation wasn't indexed
                    self.operations.remove(i);
                    self.next()
                }
            }
        } else {
            Err(OperationError::new(Status::Empty, FailureMode::default())) // No more operations
        }
    }

    #[cfg(test)]
    pub fn default() -> Self {
        OperationDispatcher {
            operations: vec![],
            waiting_operations: HashMap::default(),
            service_handlers: HashMap::default(),
        }
    }
    #[cfg(test)]
    pub fn get_current_operation_state(&self) -> Option<State> {
        self.operations
            .first()
            .map(|operation| operation.get_state())
    }
}

fn grpc_call_fn(
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

fn get_map_values_bytes_fn(map_type: MapType, key: &str) -> Result<Option<Bytes>, Status> {
    hostcalls::get_map_value_bytes(map_type, key)
}

fn grpc_message_build_fn(
    extension_type: &ServiceType,
    action: &Action,
) -> Option<GrpcMessageRequest> {
    GrpcMessageRequest::new(extension_type, action)
}

type ConditionsApplyFn = fn(action: &Action) -> bool;

fn conditions_apply_fn(action: &Action) -> bool {
    action.conditions_apply()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configuration::Timeout;
    use crate::envoy::RateLimitRequest;
    use protobuf::RepeatedField;
    use std::cell::OnceCell;
    use std::time::Duration;

    fn default_grpc_call_fn_stub(
        _upstream_name: &str,
        _service_name: &str,
        _method_name: &str,
        _initial_metadata: Vec<(&str, &[u8])>,
        _message: Option<&[u8]>,
        _timeout: Duration,
    ) -> Result<u32, Status> {
        Ok(200)
    }

    fn get_map_values_bytes_fn_stub(
        _map_type: MapType,
        _key: &str,
    ) -> Result<Option<Bytes>, Status> {
        Ok(Some(Vec::new()))
    }

    fn grpc_message_build_fn_stub(
        _extension_type: &ServiceType,
        _action: &Action,
    ) -> Option<GrpcMessageRequest> {
        Some(GrpcMessageRequest::RateLimit(build_message()))
    }

    fn build_grpc_service_handler() -> GrpcServiceHandler {
        GrpcServiceHandler::new(Rc::new(Default::default()), Rc::new(Default::default()))
    }

    fn conditions_apply_fn_stub(_action: &Action) -> bool {
        true
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

    fn build_operation(
        grpc_call_fn_stub: GrpcCallFn,
        extension_type: ServiceType,
    ) -> Rc<Operation> {
        Rc::new(Operation {
            state: RefCell::from(State::Pending),
            result: RefCell::new(Ok(0)),
            service: Rc::new(Service {
                service_type: extension_type,
                endpoint: "local".to_string(),
                failure_mode: FailureMode::Deny,
                timeout: Timeout(Duration::from_millis(42)),
            }),
            action: Action {
                service: "local".to_string(),
                scope: "".to_string(),
                conditions: vec![],
                predicates: vec![],
                compiled_predicates: OnceCell::default(),
                data: vec![],
            },
            service_handler: Rc::new(build_grpc_service_handler()),
            grpc_call_fn: grpc_call_fn_stub,
            get_map_values_bytes_fn: get_map_values_bytes_fn_stub,
            grpc_message_build_fn: grpc_message_build_fn_stub,
            conditions_apply_fn: conditions_apply_fn_stub,
        })
    }

    #[test]
    fn operation_getters() {
        let operation = build_operation(default_grpc_call_fn_stub, ServiceType::RateLimit);

        assert_eq!(operation.get_state(), State::Pending);
        assert_eq!(*operation.get_service_type(), ServiceType::RateLimit);
        assert_eq!(operation.get_failure_mode(), FailureMode::Deny);
        assert_eq!(operation.get_result(), Ok(0));
    }

    #[test]
    fn operation_transition() {
        let operation = build_operation(default_grpc_call_fn_stub, ServiceType::RateLimit);
        assert_eq!(operation.get_result(), Ok(0));
        assert_eq!(operation.get_state(), State::Pending);
        let mut res = operation.trigger();
        assert_eq!(res, Ok(200));
        assert_eq!(operation.get_state(), State::Waiting);
        res = operation.trigger();
        assert_eq!(res, Ok(200));
        assert_eq!(operation.get_result(), Ok(200));
        assert_eq!(operation.get_state(), State::Done);
    }

    #[test]
    fn operation_dispatcher_push_actions() {
        let mut operation_dispatcher = OperationDispatcher::default();

        assert_eq!(operation_dispatcher.operations.len(), 0);
        operation_dispatcher.push_operations(vec![build_operation(
            default_grpc_call_fn_stub,
            ServiceType::RateLimit,
        )]);

        assert_eq!(operation_dispatcher.operations.len(), 1);
    }

    #[test]
    fn operation_dispatcher_get_current_action_state() {
        let mut operation_dispatcher = OperationDispatcher::default();
        operation_dispatcher.push_operations(vec![build_operation(
            default_grpc_call_fn_stub,
            ServiceType::RateLimit,
        )]);
        assert_eq!(
            operation_dispatcher.get_current_operation_state(),
            Some(State::Pending)
        );
    }

    #[test]
    fn operation_dispatcher_next() {
        let mut operation_dispatcher = OperationDispatcher::default();

        fn grpc_call_fn_stub_66(
            _upstream_name: &str,
            _service_name: &str,
            _method_name: &str,
            _initial_metadata: Vec<(&str, &[u8])>,
            _message: Option<&[u8]>,
            _timeout: Duration,
        ) -> Result<u32, Status> {
            Ok(66)
        }

        fn grpc_call_fn_stub_77(
            _upstream_name: &str,
            _service_name: &str,
            _method_name: &str,
            _initial_metadata: Vec<(&str, &[u8])>,
            _message: Option<&[u8]>,
            _timeout: Duration,
        ) -> Result<u32, Status> {
            Ok(77)
        }

        operation_dispatcher.push_operations(vec![
            build_operation(grpc_call_fn_stub_66, ServiceType::RateLimit),
            build_operation(grpc_call_fn_stub_77, ServiceType::Auth),
        ]);

        assert_eq!(
            operation_dispatcher.get_current_operation_state(),
            Some(State::Pending)
        );
        assert_eq!(operation_dispatcher.waiting_operations.len(), 0);

        let mut op = operation_dispatcher.next();
        assert_eq!(op.clone().unwrap().get_result(), Ok(66));
        assert_eq!(
            *op.clone().unwrap().get_service_type(),
            ServiceType::RateLimit
        );
        assert_eq!(op.unwrap().get_state(), State::Waiting);
        assert_eq!(operation_dispatcher.waiting_operations.len(), 1);

        op = operation_dispatcher.next();
        assert_eq!(op.clone().unwrap().get_result(), Ok(66));
        assert_eq!(op.unwrap().get_state(), State::Done);

        op = operation_dispatcher.next();
        assert_eq!(op.clone().unwrap().get_result(), Ok(77));
        assert_eq!(*op.clone().unwrap().get_service_type(), ServiceType::Auth);
        assert_eq!(op.unwrap().get_state(), State::Waiting);
        assert_eq!(operation_dispatcher.waiting_operations.len(), 1);

        op = operation_dispatcher.next();
        assert_eq!(op.clone().unwrap().get_result(), Ok(77));
        assert_eq!(op.unwrap().get_state(), State::Done);
        assert_eq!(operation_dispatcher.waiting_operations.len(), 1);

        op = operation_dispatcher.next();
        assert!(op.is_err());
        assert!(operation_dispatcher.get_current_operation_state().is_none());
        assert_eq!(operation_dispatcher.waiting_operations.len(), 0);
    }
}
