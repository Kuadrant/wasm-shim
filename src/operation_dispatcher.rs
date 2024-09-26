use crate::configuration::{Extension, ExtensionType, FailureMode};
use crate::policy::Rule;
use crate::service::grpc_message::GrpcMessageRequest;
use crate::service::{GetMapValuesBytesFn, GrpcCallFn, GrpcServiceHandler};
use log::debug;
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

type Procedure = (Rc<GrpcServiceHandler>, GrpcMessageRequest);

#[allow(dead_code)]
#[derive(Clone)]
pub(crate) struct Operation {
    state: State,
    result: Result<u32, Status>,
    extension: Rc<Extension>,
    procedure: Procedure,
    grpc_call_fn: GrpcCallFn,
    get_map_values_bytes_fn: GetMapValuesBytesFn,
}

#[allow(dead_code)]
impl Operation {
    pub fn new(extension: Rc<Extension>, procedure: Procedure) -> Self {
        Self {
            state: State::Pending,
            result: Ok(0), // Heuristics: zero represents that it's not been triggered, following `hostcalls` example
            extension,
            procedure,
            grpc_call_fn,
            get_map_values_bytes_fn,
        }
    }

    fn trigger(&mut self) -> Result<u32, Status> {
        match self.state {
            State::Pending => {
                self.result = self.procedure.0.send(
                    self.get_map_values_bytes_fn,
                    self.grpc_call_fn,
                    self.procedure.1.clone(),
                );
                self.state.next();
                self.result
            }
            State::Waiting => {
                self.state.next();
                self.result
            }
            State::Done => self.result,
        }
    }

    pub fn get_state(&self) -> &State {
        &self.state
    }

    pub fn get_result(&self) -> Result<u32, Status> {
        self.result
    }

    pub fn get_extension_type(&self) -> &ExtensionType {
        &self.extension.extension_type
    }

    pub fn get_failure_mode(&self) -> &FailureMode {
        &self.extension.failure_mode
    }
}

#[allow(dead_code)]
pub struct OperationDispatcher {
    operations: RefCell<Vec<Operation>>,
    waiting_operations: RefCell<HashMap<u32, Operation>>, // TODO(didierofrivia): Maybe keep references or Rc
    service_handlers: HashMap<String, Rc<GrpcServiceHandler>>,
}

#[allow(dead_code)]
impl OperationDispatcher {
    pub fn default() -> Self {
        OperationDispatcher {
            operations: RefCell::new(vec![]),
            waiting_operations: RefCell::new(HashMap::default()),
            service_handlers: HashMap::default(),
        }
    }
    pub fn new(service_handlers: HashMap<String, Rc<GrpcServiceHandler>>) -> Self {
        Self {
            service_handlers,
            operations: RefCell::new(vec![]),
            waiting_operations: RefCell::new(HashMap::new()),
        }
    }

    pub fn get_operation(&self, token_id: u32) -> Option<Operation> {
        self.waiting_operations.borrow_mut().get(&token_id).cloned()
    }

    pub fn build_operations(&self, rule: &Rule) {
        let mut operations: Vec<Operation> = vec![];
        for action in rule.actions.iter() {
            // TODO(didierofrivia): Error handling
            if let Some(service) = self.service_handlers.get(&action.extension) {
                let descriptors = match service.get_extension_type() {
                    ExtensionType::Auth => None,
                    ExtensionType::RateLimit => {
                        let desc = action.build_descriptors();
                        if desc.is_empty() {
                            debug!("process_policy: empty descriptors");
                            continue;
                        }
                        Some(desc)
                    }
                };

                let message = GrpcMessageRequest::new(
                    service.get_extension_type(),
                    action.scope.clone(),
                    descriptors.clone(),
                );
                operations.push(Operation::new(
                    service.get_extension(),
                    (Rc::clone(service), message),
                ))
            }
        }
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
                if let Ok(token_id) = operation.result {
                    self.waiting_operations.borrow_mut().remove(&token_id);
                } // If result was Err, means the operation wasn't indexed
                operations.remove(i);
                // The next op is now at `i`
            }
            if let Some(operation) = operations.get_mut(i) {
                if let Ok(token_id) = operation.trigger() {
                    if *operation.get_state() == State::Waiting {
                        // We index only if it was just transitioned to Waiting after triggering
                        self.waiting_operations
                            .borrow_mut()
                            .insert(token_id, operation.clone());
                    } // TODO(didierofrivia): Decide on indexing the failed operations.
                }
                Some(operation.clone())
            } else {
                None
            }
        } else {
            None
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envoy::RateLimitRequest;
    use protobuf::RepeatedField;
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

    fn build_operation(grpc_call_fn_stub: GrpcCallFn, extension_type: ExtensionType) -> Operation {
        Operation {
            state: State::Pending,
            result: Ok(0),
            extension: Rc::new(Extension {
                extension_type,
                endpoint: "local".to_string(),
                failure_mode: FailureMode::Deny,
            }),
            procedure: (
                Rc::new(build_grpc_service_handler()),
                GrpcMessageRequest::RateLimit(build_message()),
            ),
            grpc_call_fn: grpc_call_fn_stub,
            get_map_values_bytes_fn: get_map_values_bytes_fn_stub,
        }
    }

    #[test]
    fn operation_getters() {
        let operation = build_operation(default_grpc_call_fn_stub, ExtensionType::RateLimit);

        assert_eq!(*operation.get_state(), State::Pending);
        assert_eq!(*operation.get_extension_type(), ExtensionType::RateLimit);
        assert_eq!(*operation.get_failure_mode(), FailureMode::Deny);
        assert_eq!(operation.get_result(), Ok(0));
    }

    #[test]
    fn operation_transition() {
        let mut operation = build_operation(default_grpc_call_fn_stub, ExtensionType::RateLimit);
        assert_eq!(operation.result, Ok(0));
        assert_eq!(*operation.get_state(), State::Pending);
        let mut res = operation.trigger();
        assert_eq!(res, Ok(200));
        assert_eq!(*operation.get_state(), State::Waiting);
        res = operation.trigger();
        assert_eq!(res, Ok(200));
        assert_eq!(operation.result, Ok(200));
        assert_eq!(*operation.get_state(), State::Done);
    }

    #[test]
    fn operation_dispatcher_push_actions() {
        let operation_dispatcher = OperationDispatcher::default();

        assert_eq!(operation_dispatcher.operations.borrow().len(), 0);
        operation_dispatcher.push_operations(vec![build_operation(
            default_grpc_call_fn_stub,
            ExtensionType::RateLimit,
        )]);

        assert_eq!(operation_dispatcher.operations.borrow().len(), 1);
    }

    #[test]
    fn operation_dispatcher_get_current_action_state() {
        let operation_dispatcher = OperationDispatcher::default();
        operation_dispatcher.push_operations(vec![build_operation(
            default_grpc_call_fn_stub,
            ExtensionType::RateLimit,
        )]);
        assert_eq!(
            operation_dispatcher.get_current_operation_state(),
            Some(State::Pending)
        );
    }

    #[test]
    fn operation_dispatcher_next() {
        let operation_dispatcher = OperationDispatcher::default();

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
            build_operation(grpc_call_fn_stub_66, ExtensionType::RateLimit),
            build_operation(grpc_call_fn_stub_77, ExtensionType::Auth),
        ]);

        assert_eq!(operation_dispatcher.get_current_operation_result(), Ok(0));
        assert_eq!(
            operation_dispatcher.get_current_operation_state(),
            Some(State::Pending)
        );
        assert_eq!(
            operation_dispatcher.waiting_operations.borrow_mut().len(),
            0
        );

        let mut op = operation_dispatcher.next();
        assert_eq!(op.clone().unwrap().get_result(), Ok(66));
        assert_eq!(
            *op.clone().unwrap().get_extension_type(),
            ExtensionType::RateLimit
        );
        assert_eq!(*op.unwrap().get_state(), State::Waiting);
        assert_eq!(
            operation_dispatcher.waiting_operations.borrow_mut().len(),
            1
        );

        op = operation_dispatcher.next();
        assert_eq!(op.clone().unwrap().get_result(), Ok(66));
        assert_eq!(*op.unwrap().get_state(), State::Done);

        op = operation_dispatcher.next();
        assert_eq!(op.clone().unwrap().get_result(), Ok(77));
        assert_eq!(
            *op.clone().unwrap().get_extension_type(),
            ExtensionType::Auth
        );
        assert_eq!(*op.unwrap().get_state(), State::Waiting);
        assert_eq!(
            operation_dispatcher.waiting_operations.borrow_mut().len(),
            1
        );

        op = operation_dispatcher.next();
        assert_eq!(op.clone().unwrap().get_result(), Ok(77));
        assert_eq!(*op.unwrap().get_state(), State::Done);
        assert_eq!(
            operation_dispatcher.waiting_operations.borrow_mut().len(),
            1
        );

        op = operation_dispatcher.next();
        assert!(op.is_none());
        assert!(operation_dispatcher.get_current_operation_state().is_none());
        assert_eq!(
            operation_dispatcher.waiting_operations.borrow_mut().len(),
            0
        );
    }
}
