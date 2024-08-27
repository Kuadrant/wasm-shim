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
#[derive(PartialEq, Clone)]
pub(crate) enum Action {
    Auth { state: State },
    RateLimit { state: State },
}

impl Action {
    pub fn trigger(&mut self) {
        match self {
            Action::Auth { .. } => self.auth(),
            Action::RateLimit { .. } => self.rate_limit(),
        }
    }

    fn get_state(&self) -> &State {
        match self {
            Action::Auth { state } => state,
            Action::RateLimit { state } => state,
        }
    }

    fn rate_limit(&mut self) {
        // Specifics for RL, returning State
        if let Action::RateLimit { state } = self {
            match state {
                State::Pending => {
                    println!("Trigger the request and return State::Waiting");
                    state.next();
                }
                State::Waiting => {
                    println!(
                        "When got on_grpc_response, process RL response and return State::Done"
                    );
                    state.next();
                }
                State::Done => {
                    println!("Done for RL... calling next action (?)");
                }
            }
        }
    }

    fn auth(&mut self) {
        // Specifics for Auth, returning State
        if let Action::Auth { state } = self {
            match state {
                State::Pending => {
                    println!("Trigger the request and return State::Waiting");
                    state.next();
                }
                State::Waiting => {
                    println!(
                        "When got on_grpc_response, process Auth response and return State::Done"
                    );
                    state.next();
                }
                State::Done => {
                    println!("Done for Auth... calling next action (?)");
                }
            }
        }
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

    pub fn new(/*vec of PluginConfig actions*/) -> ActionDispatcher {
        ActionDispatcher::default()
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
        let mut action = Action::Auth {
            state: State::Pending,
        };
        assert_eq!(*action.get_state(), State::Pending);
        action.trigger();
        assert_eq!(*action.get_state(), State::Waiting);
        action.trigger();
        assert_eq!(*action.get_state(), State::Done);
    }

    #[test]
    fn action_dispatcher_push_actions() {
        let mut action_dispatcher = ActionDispatcher {
            actions: RefCell::new(vec![Action::RateLimit {
                state: State::Pending,
            }]),
        };

        assert_eq!(action_dispatcher.actions.borrow().len(), 1);

        action_dispatcher.push_actions(vec![Action::Auth {
            state: State::Pending,
        }]);

        assert_eq!(action_dispatcher.actions.borrow().len(), 2);
    }

    #[test]
    fn action_dispatcher_get_current_action_state() {
        let action_dispatcher = ActionDispatcher {
            actions: RefCell::new(vec![Action::RateLimit {
                state: State::Waiting,
            }]),
        };

        assert_eq!(
            action_dispatcher.get_current_action_state(),
            Some(State::Waiting)
        );

        let action_dispatcher2 = ActionDispatcher::default();

        assert_eq!(action_dispatcher2.get_current_action_state(), None);
    }

    #[test]
    fn action_dispatcher_next() {
        let mut action_dispatcher = ActionDispatcher {
            actions: RefCell::new(vec![Action::RateLimit {
                state: State::Pending,
            }]),
        };
        let mut res = action_dispatcher.next();
        assert_eq!(res, true);
        assert_eq!(
            action_dispatcher.get_current_action_state(),
            Some(State::Waiting)
        );

        res = action_dispatcher.next();
        assert_eq!(res, true);
        assert_eq!(
            action_dispatcher.get_current_action_state(),
            Some(State::Done)
        );

        res = action_dispatcher.next();
        assert_eq!(res, false);
        assert_eq!(action_dispatcher.get_current_action_state(), None);
    }
}
