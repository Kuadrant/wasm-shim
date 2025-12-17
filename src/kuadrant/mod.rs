mod cache;
mod context;
mod pipeline;
mod resolver;

use std::cell::RefCell;
use std::sync::Arc;
#[cfg(test)]
pub use resolver::MockWasmHost;

#[cfg(test)]
thread_local! {
    pub static MOCK: RefCell<Option<Arc<MockWasmHost>>> = RefCell::new(None);
}

pub(crate) use cache::CachedValue;
pub(crate) use context::ReqRespCtx;
pub(crate) use pipeline::{Pipeline, PipelineFactory, PipelineState};
