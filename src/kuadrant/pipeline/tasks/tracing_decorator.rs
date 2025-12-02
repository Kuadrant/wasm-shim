use crate::kuadrant::pipeline::tasks::{Task, TaskOutcome};
use crate::kuadrant::ReqRespCtx;

pub struct TracingDecoratorTask {
    pub task: Box<dyn Task>,
    pub span: Option<tracing::Span>,
    pub span_name: &'static str,
    pub sources: Vec<String>,
}

impl TracingDecoratorTask {
    pub fn new(span_name: &'static str, task: Box<dyn Task>, sources: Vec<String>) -> Self {
        Self {
            task,
            span: None,
            span_name,
            sources,
        }
    }

    fn ensure_span(&mut self) -> &tracing::Span {
        self.span.get_or_insert_with(|| {
            let span = tracing::info_span!(
                "task_wrapper",
                task_id = ?self.task.id(),
                sources = ?self.sources,
                otel.name = tracing::field::Empty,
                otel.status_code = tracing::field::Empty,
                otel.status_message = tracing::field::Empty
            );
            span.record("otel.name", self.span_name);
            span
        })
    }
}

impl Task for TracingDecoratorTask {
    fn apply(mut self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        let span = self.ensure_span().clone();
        let _guard = span.enter();

        match self.task.apply(ctx) {
            TaskOutcome::Deferred { token_id, pending } => TaskOutcome::Deferred {
                token_id,
                pending: Box::new(TracingDecoratorTask {
                    task: pending,
                    span: Some(span.clone()),
                    span_name: self.span_name,
                    sources: self.sources,
                }),
            },
            TaskOutcome::Requeued(tasks) => {
                let wrapped = tasks
                    .into_iter()
                    .map(|task| {
                        Box::new(TracingDecoratorTask {
                            task,
                            span: Some(span.clone()),
                            span_name: self.span_name,
                            sources: self.sources.clone(),
                        }) as Box<dyn Task>
                    })
                    .collect();
                TaskOutcome::Requeued(wrapped)
            }
            TaskOutcome::Terminate(task) => {
                TaskOutcome::Terminate(Box::new(TracingDecoratorTask {
                    task,
                    span: Some(span.clone()),
                    span_name: self.span_name,
                    sources: self.sources.clone(),
                }))
            }
            outcome => outcome,
        }
    }

    fn id(&self) -> Option<String> {
        self.task.id()
    }

    fn dependencies(&self) -> &[String] {
        self.task.dependencies()
    }

    fn pauses_filter(&self) -> bool {
        self.task.pauses_filter()
    }
}
