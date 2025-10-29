use crate::v2::kuadrant::pipeline::tasks::{PendingTask, ResponseProcessor, Task, TaskOutcome};
use crate::v2::kuadrant::ReqRespCtx;
use crate::v2::services::Service;
use std::rc::Rc;

#[allow(dead_code)]
pub struct SendTask<S: Service> {
    task_id: String,
    dependencies: Vec<String>,
    is_blocking: bool,
    service: Rc<S>,
    message: Vec<u8>,
    process_response: Box<ResponseProcessor<S::Response>>,
}

impl<S: Service> SendTask<S> {
    pub fn new(
        task_id: String,
        dependencies: Vec<String>,
        is_blocking: bool,
        service: Rc<S>,
        message: Vec<u8>,
        process_response: Box<ResponseProcessor<S::Response>>,
    ) -> Self {
        Self {
            task_id,
            dependencies,
            is_blocking,
            service,
            message,
            process_response,
        }
    }
}

impl<S: Service + 'static> Task for SendTask<S> {
    fn id(&self) -> Option<String> {
        Some(self.task_id.clone())
    }

    fn dependencies(&self) -> &[String] {
        self.dependencies.as_slice()
    }

    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        let token_id = match self.service.dispatch(ctx, self.message) {
            Ok(id) => id,
            Err(_e) => {
                // todo(refactor): error handling
                return TaskOutcome::Failed;
            }
        };
        let service = self.service.clone();
        let process_response = self.process_response;

        TaskOutcome::Deferred {
            token_id,
            pending: PendingTask {
                task_id: Some(self.task_id),
                is_blocking: self.is_blocking,
                process_response: Box::new(move |response| {
                    match service.parse_message(response) {
                        Ok(parsed) => process_response(parsed),
                        Err(_e) => {
                            // todo(refactor): error handling
                            Vec::new()
                        }
                    }
                }),
            },
        }
    }
}
