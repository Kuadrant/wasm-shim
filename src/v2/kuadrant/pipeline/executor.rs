use crate::v2::kuadrant::ReqRespCtx;

pub(super) struct Pipeline {
    ctx: ReqRespCtx,
}

impl Pipeline {
    pub fn new(ctx: ReqRespCtx) -> Self {
        Self { ctx }
    }
}
