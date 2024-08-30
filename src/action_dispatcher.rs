use proxy_wasm::types::Status;
use std::cell::RefCell;

#[derive(PartialEq, Debug, Clone)]
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
}
#[derive(Clone)]
pub(crate) struct Action {
    state: State,
    result: Result<u32, Status>,
    operation: Option<fn() -> Result<u32, Status>>,
}

impl Action {
    pub fn default() -> Self {
        Self {
            state: State::Pending,
            result: Err(Status::Empty),
            operation: None,
        }
    }

    pub fn set_operation(&mut self, operation: fn() -> Result<u32, Status>) {
        self.operation = Some(operation);
    }

    pub fn trigger(&mut self) {
        if let State::Done = self.state {
        } else if let Some(operation) = self.operation {
            self.result = operation();
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

pub struct ActionDispatcher {
    actions: RefCell<Vec<Action>>,
}

impl ActionDispatcher {
    pub fn default() -> ActionDispatcher {
        ActionDispatcher {
            actions: RefCell::new(vec![]),
        }
    }

    pub fn push_actions(&self, actions: Vec<Action>) {
        self.actions.borrow_mut().extend(actions);
    }

    pub fn get_current_action_state(&self) -> Option<State> {
        self.actions
            .borrow()
            .first()
            .map(|action| action.get_state().clone())
    }

    pub fn get_current_action_result(&self) -> Result<u32, Status> {
        self.actions.borrow().first().unwrap().get_result()
    }

    pub fn next(&self) -> bool {
        let mut actions = self.actions.borrow_mut();
        if let Some((i, action)) = actions.iter_mut().enumerate().next() {
            if let State::Done = action.get_state() {
                actions.remove(i);
                actions.len() > 0
            } else {
                action.trigger();
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
    fn action_transition() {
        let mut action = Action::default();
        action.set_operation(|| -> Result<u32, Status> { Ok(200) });
        assert_eq!(action.get_state(), State::Pending);
        action.trigger();
        assert_eq!(action.get_state(), State::Waiting);
        action.trigger();
        assert_eq!(action.result, Ok(200));
        assert_eq!(action.get_state(), State::Done);
    }

    #[test]
    fn action_dispatcher_push_actions() {
        let action_dispatcher = ActionDispatcher {
            actions: RefCell::new(vec![Action::default()]),
        };

        assert_eq!(action_dispatcher.actions.borrow().len(), 1);

        action_dispatcher.push_actions(vec![Action::default()]);

        assert_eq!(action_dispatcher.actions.borrow().len(), 2);
    }

    #[test]
    fn action_dispatcher_get_current_action_state() {
        let action_dispatcher = ActionDispatcher {
            actions: RefCell::new(vec![Action::default()]),
        };

        assert_eq!(
            action_dispatcher.get_current_action_state(),
            Some(State::Pending)
        );
    }

    #[test]
    fn action_dispatcher_next() {
        let mut action = Action::default();
        action.set_operation(|| -> Result<u32, Status> { Ok(200) });
        let action_dispatcher = ActionDispatcher {
            actions: RefCell::new(vec![action]),
        };
        let mut res = action_dispatcher.next();
        assert!(res);
        assert_eq!(
            action_dispatcher.get_current_action_state(),
            Some(State::Waiting)
        );

        res = action_dispatcher.next();
        assert!(res);
        assert_eq!(
            action_dispatcher.get_current_action_state(),
            Some(State::Done)
        );
        assert_eq!(action_dispatcher.get_current_action_result(), Ok(200));

        res = action_dispatcher.next();
        assert!(!res);
        assert_eq!(action_dispatcher.get_current_action_state(), None);
    }
}
