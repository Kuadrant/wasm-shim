use crate::configuration::{ActionSet, Service};
use crate::data::{Predicate, PredicateResult, PredicateVec};
use crate::runtime_action::errors::ActionCreationError;
use crate::runtime_action::RuntimeAction;
use crate::service::{GrpcErrResponse, HeaderKind, IndexedGrpcRequest};
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Debug)]
pub struct RuntimeActionSet {
    pub name: String,
    pub route_rule_predicates: Vec<Predicate>,
    pub runtime_actions: Vec<Rc<RuntimeAction>>,
}

pub type IndexedRequestResult = Result<Option<IndexedGrpcRequest>, GrpcErrResponse>;

impl RuntimeActionSet {
    pub fn new(
        action_set: &ActionSet,
        services: &HashMap<String, Service>,
    ) -> Result<Self, ActionCreationError> {
        // route predicates
        let mut route_rule_predicates = Vec::default();
        for predicate in &action_set.route_rule_conditions.predicates {
            route_rule_predicates.push(Predicate::route_rule(predicate)?);
        }

        // actions
        let mut all_runtime_actions = Vec::default();
        for action in action_set.actions.iter() {
            all_runtime_actions.push(RuntimeAction::new(action, services)?);
        }
        let runtime_actions = Self::merge_subsequent_actions_of_a_kind(all_runtime_actions)?;

        Ok(Self {
            name: action_set.name.clone(),
            route_rule_predicates,
            runtime_actions: runtime_actions.into_iter().map(Rc::new).collect(),
        })
    }

    fn merge_subsequent_actions_of_a_kind(
        runtime_actions: Vec<RuntimeAction>,
    ) -> Result<Vec<RuntimeAction>, ActionCreationError> {
        let mut folded_actions: Vec<RuntimeAction> = Vec::default();
        for r_action in runtime_actions {
            match folded_actions.last_mut() {
                Some(existing_action) => match existing_action.merge(r_action) {
                    Ok(None) => {}
                    Ok(Some(unmerged_action)) => {
                        folded_actions.push(unmerged_action);
                    }
                    Err(e) => return Err(e),
                },
                None => folded_actions.push(r_action),
            }
        }
        Ok(folded_actions)
    }

    pub fn conditions_apply(&self) -> PredicateResult {
        self.route_rule_predicates.apply()
    }

    pub fn find_first_grpc_request(&self) -> IndexedRequestResult {
        self.find_next_grpc_request(0)
    }

    pub fn find_next_grpc_request(&self, start: usize) -> IndexedRequestResult {
        for (index, action) in self.runtime_actions.iter().skip(start).enumerate() {
            match action.process_request() {
                Ok(Some(request)) => {
                    return Ok(Some(IndexedGrpcRequest::new(start + index, request)))
                }
                Ok(None) => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(None)
    }

    pub fn process_grpc_response(
        &self,
        index: usize,
        msg: &[u8],
    ) -> Result<(IndexedRequestResult, HeaderKind), GrpcErrResponse> {
        self.runtime_actions[index]
            .process_response(msg)
            .map(|headers| {
                let next_msg = self.find_next_grpc_request(index + 1);
                (next_msg, headers)
            })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::configuration::{
        Action, ActionSet, FailureMode, RouteRuleConditions, ServiceType, Timeout,
    };

    #[test]
    fn empty_route_rule_predicates_do_apply() {
        let action_set = ActionSet::new("some_name".to_owned(), Default::default(), Vec::new());

        let runtime_action_set = RuntimeActionSet::new(&action_set, &HashMap::default())
            .expect("should not happen from an empty set of actions");
        assert_eq!(runtime_action_set.conditions_apply(), Ok(true));
    }

    #[test]
    fn when_all_predicates_are_truthy_conditions_apply() {
        let action_set = ActionSet::new(
            "some_name".to_owned(),
            RouteRuleConditions {
                hostnames: Vec::default(),
                predicates: vec!["true".into(), "true".into()],
            },
            Vec::new(),
        );

        let runtime_action_set = RuntimeActionSet::new(&action_set, &HashMap::default())
            .expect("should not happen from an empty set of actions");
        assert_eq!(runtime_action_set.conditions_apply(), Ok(true));
    }

    #[test]
    fn when_not_all_predicates_are_truthy_action_does_not_apply() {
        let action_set = ActionSet::new(
            "some_name".to_owned(),
            RouteRuleConditions {
                hostnames: Vec::default(),
                predicates: vec!["true".into(), "true".into(), "true".into(), "false".into()],
            },
            Vec::new(),
        );

        let runtime_action_set = RuntimeActionSet::new(&action_set, &HashMap::default())
            .expect("should not happen from an empty set of actions");
        assert_eq!(runtime_action_set.conditions_apply(), Ok(false));
    }

    #[test]
    fn when_a_cel_expression_does_not_evaluate_to_bool_returns_error() {
        let action_set = ActionSet::new(
            "some_name".to_owned(),
            RouteRuleConditions {
                hostnames: Vec::default(),
                predicates: vec!["true".into(), "true".into(), "true".into(), "1".into()],
            },
            Vec::new(),
        );

        let runtime_action_set = RuntimeActionSet::new(&action_set, &HashMap::default())
            .expect("should not happen from an empty set of actions");
        assert!(runtime_action_set.conditions_apply().is_err());
    }

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
    fn simple_folding() {
        let action_a = build_action("rl_service_common", "scope_common");
        let action_b = build_action("rl_service_common", "scope_common");

        let action_set = ActionSet::new(
            "some_name".to_owned(),
            Default::default(),
            vec![action_a, action_b],
        );

        let mut services = HashMap::new();
        services.insert(String::from("rl_service_common"), build_rl_service());
        let runtime_action_set = RuntimeActionSet::new(&action_set, &services)
            .expect("should not happen for simple actions");

        assert_eq!(runtime_action_set.runtime_actions.len(), 1);
    }

    #[test]
    fn unrelated_actions_by_kind_are_not_folded() {
        let red_action_0 = build_action("service_red", "scope_red");
        let blue_action_1 = build_action("service_blue", "scope_blue");

        let action_set = ActionSet::new(
            "some_name".to_owned(),
            Default::default(),
            vec![red_action_0, blue_action_1],
        );

        let mut services = HashMap::new();
        services.insert(String::from("service_red"), build_rl_service());
        services.insert(String::from("service_blue"), build_auth_service());

        let runtime_action_set = RuntimeActionSet::new(&action_set, &services)
            .expect("should not happen from simple actions");

        assert_eq!(runtime_action_set.runtime_actions.len(), 2);
    }

    #[test]
    fn unrelated_rl_actions_are_not_folded() {
        let red_action_0 = build_action("service_red", "scope_red");
        let blue_action_1 = build_action("service_blue", "scope_blue");
        let green_action_2 = build_action("service_green", "scope_green");

        let action_set = ActionSet::new(
            "some_name".to_owned(),
            Default::default(),
            vec![red_action_0, blue_action_1, green_action_2],
        );

        let mut services = HashMap::new();
        services.insert(String::from("service_red"), build_rl_service());
        services.insert(String::from("service_blue"), build_rl_service());
        services.insert(String::from("service_green"), build_rl_service());

        let runtime_action_set = RuntimeActionSet::new(&action_set, &services)
            .expect("should not happen from simple actions");

        assert_eq!(runtime_action_set.runtime_actions.len(), 3);
    }

    #[test]
    fn only_subsequent_actions_are_folded() {
        let red_action_0 = build_action("service_red", "common");
        let red_action_1 = build_action("service_red", "common");
        let blue_action_2 = build_action("service_blue", "common");
        let red_action_3 = build_action("service_red", "common");
        let red_action_4 = build_action("service_red", "common");

        let action_set = ActionSet::new(
            "some_name".to_owned(),
            Default::default(),
            vec![
                red_action_0,
                red_action_1,
                blue_action_2,
                red_action_3,
                red_action_4,
            ],
        );

        let mut services = HashMap::new();
        services.insert(String::from("service_red"), build_rl_service());
        services.insert(String::from("service_blue"), build_rl_service());

        let runtime_action_set = RuntimeActionSet::new(&action_set, &services)
            .expect("should not happen from simple actions");

        assert_eq!(runtime_action_set.runtime_actions.len(), 3);
    }
}
