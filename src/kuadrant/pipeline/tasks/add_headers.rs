use crate::data::attribute::AttributeState;
use crate::data::cel::{Expression, Predicate, PredicateVec};
use crate::kuadrant::pipeline::tasks::{
    HeaderOperation, HeadersType, ModifyHeadersTask, Task, TaskOutcome,
};
use crate::kuadrant::ReqRespCtx;
use cel::objects::Key;
use cel::Value;
use tracing::{debug, error};

pub struct AddHeadersTask {
    task_id: String,
    predicates: Vec<Predicate>,
    headers_to_add: Expression,
    dependencies: Vec<String>,
}

impl AddHeadersTask {
    pub fn new(
        task_id: String,
        predicates: Vec<Predicate>,
        headers_to_add: Expression,
        dependencies: Vec<String>,
    ) -> Self {
        Self {
            task_id,
            predicates,
            headers_to_add,
            dependencies,
        }
    }
}

impl Task for AddHeadersTask {
    fn id(&self) -> Option<String> {
        Some(self.task_id.clone())
    }

    fn dependencies(&self) -> &[String] {
        &self.dependencies
    }

    fn pauses_filter(&self) -> bool {
        false
    }

    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        match self.predicates.apply(ctx) {
            Ok(AttributeState::Pending) => return TaskOutcome::Requeued(vec![self]),
            Ok(AttributeState::Available(false)) => return TaskOutcome::Done,
            Ok(AttributeState::Available(true)) => {}
            Err(e) => {
                error!("AddHeadersTask predicates failed: {e:?}");
                return TaskOutcome::Failed;
            }
        }

        match self.headers_to_add.eval(ctx) {
            Ok(AttributeState::Pending) => TaskOutcome::Requeued(vec![self]),
            Ok(AttributeState::Available(value)) => {
                let headers = match extract_headers(&value) {
                    Some(h) => h,
                    None => {
                        error!(
                            "AddHeadersTask: headers_to_add did not evaluate to a map of strings"
                        );
                        return TaskOutcome::Failed;
                    }
                };
                debug!("AddHeadersTask adding {} headers", headers.len());
                TaskOutcome::Requeued(vec![Box::new(ModifyHeadersTask::new(
                    HeaderOperation::Append(headers.into()),
                    HeadersType::HttpResponseHeaders,
                ))])
            }
            Err(e) => {
                error!("AddHeadersTask headers_to_add evaluation failed: {e:?}");
                TaskOutcome::Failed
            }
        }
    }
}

fn extract_headers(value: &Value) -> Option<Vec<(String, String)>> {
    match value {
        Value::Map(map) => {
            let mut headers = Vec::new();
            for (key, val) in map.map.iter() {
                let key_str = match key {
                    Key::String(s) => s.to_string(),
                    _ => return None,
                };
                let val_str = match val {
                    Value::String(s) => s.to_string(),
                    Value::Int(n) => format!("{n}"),
                    Value::UInt(n) => format!("{n}"),
                    Value::Float(n) => format!("{n}"),
                    Value::Bool(b) => format!("{b}"),
                    _ => return None,
                };
                headers.push((key_str, val_str));
            }
            Some(headers)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kuadrant::resolver::MockWasmHost;
    use std::sync::Arc;

    #[test]
    fn add_headers_task_evaluates_and_requeues_modify() {
        let existing_headers = vec![("existing".to_string(), "value".to_string())];
        let mock_host =
            MockWasmHost::new().with_map("response.headers".to_string(), existing_headers);
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));

        let task = Box::new(AddHeadersTask::new(
            "test".to_string(),
            vec![],
            Expression::new(r#"{"x-custom": "added"}"#).unwrap(),
            vec![],
        ));

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Requeued(_)));
    }

    #[test]
    fn add_headers_task_predicate_false_skips() {
        let mock_host = MockWasmHost::new();
        let mut ctx = ReqRespCtx::new(Arc::new(mock_host));

        let task = Box::new(AddHeadersTask::new(
            "test".to_string(),
            vec![Predicate::new("false").unwrap()],
            Expression::new(r#"{"x-custom": "added"}"#).unwrap(),
            vec![],
        ));

        let outcome = task.apply(&mut ctx);
        assert!(matches!(outcome, TaskOutcome::Done));
    }
}
