use crate::configuration::{FailureMode, ServiceType};
use crate::runtime_action::RuntimeAction;
use crate::service::grpc_message::GrpcMessageRequest;
use crate::service::{
    GetMapValuesBytesFn, GrpcCallFn, GrpcMessageBuildFn, GrpcServiceHandler, HeaderResolver,
    ServiceMetrics,
};
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

#[derive(Debug)]
pub(crate) struct Operation {
    state: RefCell<State>,
    result: RefCell<Result<u32, OperationError>>,
    action: Rc<RuntimeAction>,
    service_handler: GrpcServiceHandler,
    service_metrics: Rc<ServiceMetrics>,
    grpc_call_fn: GrpcCallFn,
    get_map_values_bytes_fn: GetMapValuesBytesFn,
    grpc_message_build_fn: GrpcMessageBuildFn,
    conditions_apply_fn: ConditionsApplyFn,
}

impl Operation {
    pub fn new(
        action: Rc<RuntimeAction>,
        service_handler: GrpcServiceHandler,
        service_metrics: &Rc<ServiceMetrics>,
    ) -> Self {
        Self {
            state: RefCell::new(State::Pending),
            result: RefCell::new(Ok(0)), // Heuristics: zero represents that it's not been triggered, following `hostcalls` example
            action,
            service_handler,
            service_metrics: Rc::clone(service_metrics),
            grpc_call_fn,
            get_map_values_bytes_fn,
            grpc_message_build_fn,
            conditions_apply_fn,
        }
    }

    fn trigger(&self) -> Result<u32, OperationError> {
        if let Some(message) = (self.grpc_message_build_fn)(&self.action) {
            let res = self.service_handler.send(
                self.get_map_values_bytes_fn,
                self.grpc_call_fn,
                message,
                self.action.get_timeout(),
            );
            match res {
                Ok(token_id) => self.set_result(Ok(token_id)),
                Err(status) => {
                    match self.get_failure_mode() {
                        FailureMode::Deny => self.get_service_metrics().report_error(),
                        FailureMode::Allow => {
                            self.get_service_metrics().report_allowed_on_failure()
                        }
                    }
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

    pub fn get_service_type(&self) -> ServiceType {
        self.action.get_service_type()
    }

    pub fn get_failure_mode(&self) -> FailureMode {
        self.action.get_failure_mode()
    }

    pub fn get_service_metrics(&self) -> &ServiceMetrics {
        &self.service_metrics
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
    header_resolver: Rc<HeaderResolver>,
    auth_service_metrics: Rc<ServiceMetrics>,
    rl_service_metrics: Rc<ServiceMetrics>,
}

impl OperationDispatcher {
    pub fn new(
        header_resolver: Rc<HeaderResolver>,
        auth_service_metrics: &Rc<ServiceMetrics>,
        rl_service_metrics: &Rc<ServiceMetrics>,
    ) -> Self {
        Self {
            operations: vec![],
            waiting_operations: HashMap::new(),
            header_resolver: Rc::clone(&header_resolver),
            auth_service_metrics: Rc::clone(auth_service_metrics),
            rl_service_metrics: Rc::clone(rl_service_metrics),
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

    pub fn build_operations(&mut self, actions: &[Rc<RuntimeAction>]) {
        let mut operations: Vec<Rc<Operation>> = vec![];
        for action in actions.iter() {
            operations.push(Rc::new(Operation::new(
                Rc::clone(action),
                GrpcServiceHandler::new(action.grpc_service(), Rc::clone(&self.header_resolver)),
                self.service_metrics_from_action(&action),
            )));
        }
        self.push_operations(operations);
    }

    pub fn push_operations(&mut self, operations: Vec<Rc<Operation>>) {
        self.operations.extend(operations);
    }

    pub fn next(&mut self) -> Result<Option<Rc<Operation>>, OperationError> {
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
                                        Ok(Some(operation.clone()))
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
                    Ok(Some(operation.clone()))
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
            Ok(None)
        }
    }

    fn service_metrics_from_action(&self, action: &RuntimeAction) -> &Rc<ServiceMetrics> {
        match action {
            RuntimeAction::Auth(_) => &self.auth_service_metrics,
            RuntimeAction::RateLimit(_) => &self.rl_service_metrics,
        }
    }

    #[cfg(test)]
    pub fn default() -> Self {
        OperationDispatcher {
            operations: vec![],
            waiting_operations: HashMap::default(),
            header_resolver: Rc::new(HeaderResolver::default()),
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

fn grpc_message_build_fn(action: &RuntimeAction) -> Option<GrpcMessageRequest> {
    GrpcMessageRequest::new(action)
}

type ConditionsApplyFn = fn(action: &RuntimeAction) -> bool;

fn conditions_apply_fn(action: &RuntimeAction) -> bool {
    action.conditions_apply()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth_action::AuthAction;
    use crate::configuration::{Action, Service, Timeout};
    use crate::envoy::RateLimitRequest;
    use crate::ratelimit_action::RateLimitAction;
    use protobuf::RepeatedField;
    use std::rc::Rc;
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

    fn grpc_message_build_fn_stub(_action: &RuntimeAction) -> Option<GrpcMessageRequest> {
        Some(GrpcMessageRequest::RateLimit(build_message()))
    }

    fn build_grpc_service_handler() -> GrpcServiceHandler {
        GrpcServiceHandler::new(Rc::new(Default::default()), Default::default())
    }

    fn conditions_apply_fn_stub(_action: &RuntimeAction) -> bool {
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

    fn build_auth_grpc_action() -> RuntimeAction {
        let service = Service {
            service_type: ServiceType::Auth,
            endpoint: "local".to_string(),
            failure_mode: FailureMode::Deny,
            timeout: Timeout(Duration::from_millis(42)),
        };
        let action = Action {
            service: "local".to_string(),
            scope: "".to_string(),
            predicates: vec![],
            data: vec![],
        };
        RuntimeAction::Auth(
            AuthAction::new(&action, &service).expect("empty predicates should compile!"),
        )
    }

    fn build_rate_limit_grpc_action() -> RuntimeAction {
        let service = Service {
            service_type: ServiceType::RateLimit,
            endpoint: "local".to_string(),
            failure_mode: FailureMode::Deny,
            timeout: Timeout(Duration::from_millis(42)),
        };
        let action = Action {
            service: "local".to_string(),
            scope: "".to_string(),
            predicates: vec![],
            data: vec![],
        };
        RuntimeAction::RateLimit(
            RateLimitAction::new(&action, &service).expect("empty predicates should compile!"),
        )
    }

    fn build_operation(grpc_call_fn_stub: GrpcCallFn, action: RuntimeAction) -> Rc<Operation> {
        Rc::new(Operation {
            state: RefCell::from(State::Pending),
            result: RefCell::new(Ok(0)),
            action: Rc::new(action),
            service_handler: build_grpc_service_handler(),
            grpc_call_fn: grpc_call_fn_stub,
            get_map_values_bytes_fn: get_map_values_bytes_fn_stub,
            grpc_message_build_fn: grpc_message_build_fn_stub,
            conditions_apply_fn: conditions_apply_fn_stub,
        })
    }

    #[test]
    fn operation_getters() {
        let operation = build_operation(default_grpc_call_fn_stub, build_rate_limit_grpc_action());

        assert_eq!(operation.get_state(), State::Pending);
        assert_eq!(operation.get_service_type(), ServiceType::RateLimit);
        assert_eq!(operation.get_failure_mode(), FailureMode::Deny);
        assert_eq!(operation.get_result(), Ok(0));
    }

    #[test]
    fn operation_transition() {
        let operation = build_operation(default_grpc_call_fn_stub, build_rate_limit_grpc_action());
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
        let operation = build_operation(default_grpc_call_fn_stub, build_rate_limit_grpc_action());
        operation_dispatcher.push_operations(vec![operation]);

        assert_eq!(operation_dispatcher.operations.len(), 1);
    }

    #[test]
    fn operation_dispatcher_get_current_action_state() {
        let mut operation_dispatcher = OperationDispatcher::default();
        let operation = build_operation(default_grpc_call_fn_stub, build_rate_limit_grpc_action());
        operation_dispatcher.push_operations(vec![operation]);
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
            build_operation(grpc_call_fn_stub_66, build_rate_limit_grpc_action()),
            build_operation(grpc_call_fn_stub_77, build_auth_grpc_action()),
        ]);

        assert_eq!(
            operation_dispatcher.get_current_operation_state(),
            Some(State::Pending)
        );
        assert_eq!(operation_dispatcher.waiting_operations.len(), 0);

        let mut op = operation_dispatcher.next();
        assert_eq!(
            op.clone()
                .expect("ok result")
                .expect("operation is some")
                .get_result(),
            Ok(66)
        );
        assert_eq!(
            op.clone()
                .expect("ok result")
                .expect("operation is some")
                .get_service_type(),
            ServiceType::RateLimit
        );
        assert_eq!(
            op.expect("ok result")
                .expect("operation is some")
                .get_state(),
            State::Waiting
        );
        assert_eq!(operation_dispatcher.waiting_operations.len(), 1);

        op = operation_dispatcher.next();
        assert_eq!(
            op.clone()
                .expect("ok result")
                .expect("operation is some")
                .get_result(),
            Ok(66)
        );
        assert_eq!(
            op.expect("ok result")
                .expect("operation is some")
                .get_state(),
            State::Done
        );

        op = operation_dispatcher.next();
        assert_eq!(
            op.clone()
                .expect("ok result")
                .expect("operation is some")
                .get_result(),
            Ok(77)
        );
        assert_eq!(
            op.clone()
                .expect("ok result")
                .expect("operation is some")
                .get_service_type(),
            ServiceType::Auth
        );
        assert_eq!(
            op.expect("ok result")
                .expect("operation is some")
                .get_state(),
            State::Waiting
        );
        assert_eq!(operation_dispatcher.waiting_operations.len(), 1);

        op = operation_dispatcher.next();
        assert_eq!(
            op.clone()
                .expect("ok result")
                .expect("operation is some")
                .get_result(),
            Ok(77)
        );
        assert_eq!(
            op.expect("ok result")
                .expect("operation is some")
                .get_state(),
            State::Done
        );
        assert_eq!(operation_dispatcher.waiting_operations.len(), 1);

        op = operation_dispatcher.next();
        assert!(op.expect("ok result").is_none());
        assert!(operation_dispatcher.get_current_operation_state().is_none());
        assert_eq!(operation_dispatcher.waiting_operations.len(), 0);
    }
}
