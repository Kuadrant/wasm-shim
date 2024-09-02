use proxy_wasm::types::Status;
use std::cell::RefCell;

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

#[allow(dead_code)]
#[derive(Clone)]
pub(crate) struct Operation {
    state: State,
    result: Result<u32, Status>,
    action: Option<fn() -> Result<u32, Status>>,
}

#[allow(dead_code)]
impl Operation {
    pub fn default() -> Self {
        Self {
            state: State::Pending,
            result: Err(Status::Empty),
            action: None,
        }
    }

    pub fn set_action(&mut self, action: fn() -> Result<u32, Status>) {
        self.action = Some(action);
    }

    pub fn trigger(&mut self) {
        if let State::Done = self.state {
        } else if let Some(action) = self.action {
            self.result = action();
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
}

#[allow(dead_code)]
impl OperationDispatcher {
    pub fn default() -> OperationDispatcher {
        OperationDispatcher {
            operations: RefCell::new(vec![]),
        }
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

    pub fn next(&self) -> bool {
        let mut operations = self.operations.borrow_mut();
        if let Some((i, operation)) = operations.iter_mut().enumerate().next() {
            if let State::Done = operation.get_state() {
                operations.remove(i);
                operations.len() > 0
            } else {
                operation.trigger();
                true
            }
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_transition() {
        let mut operation = Operation::default();
        operation.set_action(|| -> Result<u32, Status> { Ok(200) });
        assert_eq!(operation.get_state(), State::Pending);
        operation.trigger();
        assert_eq!(operation.get_state(), State::Waiting);
        operation.trigger();
        assert_eq!(operation.result, Ok(200));
        assert_eq!(operation.get_state(), State::Done);
    }

    #[test]
    fn operation_dispatcher_push_actions() {
        let operation_dispatcher = OperationDispatcher {
            operations: RefCell::new(vec![Operation::default()]),
        };

        assert_eq!(operation_dispatcher.operations.borrow().len(), 1);

        operation_dispatcher.push_operations(vec![Operation::default()]);

        assert_eq!(operation_dispatcher.operations.borrow().len(), 2);
    }

    #[test]
    fn operation_dispatcher_get_current_action_state() {
        let operation_dispatcher = OperationDispatcher {
            operations: RefCell::new(vec![Operation::default()]),
        };

        assert_eq!(
            operation_dispatcher.get_current_operation_state(),
            Some(State::Pending)
        );
    }

    #[test]
    fn operation_dispatcher_next() {
        let mut operation = Operation::default();
        operation.set_action(|| -> Result<u32, Status> { Ok(200) });
        let operation_dispatcher = OperationDispatcher {
            operations: RefCell::new(vec![operation]),
        };
        let mut res = operation_dispatcher.next();
        assert!(res);
        assert_eq!(
            operation_dispatcher.get_current_operation_state(),
            Some(State::Waiting)
        );

        res = operation_dispatcher.next();
        assert!(res);
        assert_eq!(
            operation_dispatcher.get_current_operation_state(),
            Some(State::Done)
        );
        assert_eq!(operation_dispatcher.get_current_operation_result(), Ok(200));

        res = operation_dispatcher.next();
        assert!(!res);
        assert_eq!(operation_dispatcher.get_current_operation_state(), None);
    }
}
