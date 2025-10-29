use crate::envoy::{check_response, CheckResponse};
use crate::v2::data::attribute::{AttributeError, AttributeState};
use crate::v2::data::cel::{Predicate, PredicateVec};
use crate::v2::kuadrant::pipeline::tasks::{Task, TaskOutcome};
use crate::v2::kuadrant::ReqRespCtx;
use crate::v2::services::AuthService;
use std::rc::Rc;

use super::send::SendTask;

pub struct AuthTask {
    task_id: String,
    service: Rc<AuthService>,
    scope: String,
    predicates: Vec<Predicate>,
    dependencies: Vec<String>,
    is_blocking: bool,
}

impl AuthTask {
    pub fn new(
        task_id: String,
        service: Rc<AuthService>,
        scope: String,
        predicates: Vec<Predicate>,
        dependencies: Vec<String>,
        is_blocking: bool,
    ) -> Self {
        Self {
            task_id,
            service,
            scope,
            predicates,
            dependencies,
            is_blocking,
        }
    }
}

impl Task for AuthTask {
    fn id(&self) -> Option<String> {
        Some(self.task_id.clone())
    }

    fn dependencies(&self) -> &[String] {
        &self.dependencies
    }

    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        match self.predicates.apply(ctx) {
            Ok(AttributeState::Pending) => return TaskOutcome::Requeued(self),
            Ok(AttributeState::Available(false)) => return TaskOutcome::Done,
            Ok(AttributeState::Available(true)) => {}
            Err(_e) => {
                return TaskOutcome::Failed;
            }
        }

        let message = match self.service.build_request(ctx, &self.scope) {
            Ok(msg) => msg,
            Err(AttributeError::NotAvailable(_)) => return TaskOutcome::Requeued(self),
            Err(_e) => {
                return TaskOutcome::Failed;
            }
        };

        let send_task = SendTask::new(
            self.task_id,
            self.dependencies,
            self.is_blocking,
            self.service.clone(),
            message,
            Box::new(process_auth_response),
        );

        TaskOutcome::Continue(Box::new(send_task))
    }
}

fn process_auth_response(response: CheckResponse) -> Vec<Box<dyn Task>> {
    let mut tasks: Vec<Box<dyn Task>> = Vec::new();

    // todo(refactor): Store dynamic_metadata

    match response.http_response {
        None => {
            // todo(refactor): Handle empty response
        }
        Some(check_response::HttpResponse::OkResponse(_ok_response)) => {
            // todo(refactor): Add headers, handle headers_to_remove
        }
        Some(check_response::HttpResponse::DeniedResponse(_denied_response)) => {
            // todo(refactor): Send direct response
        }
    }

    tasks
}
