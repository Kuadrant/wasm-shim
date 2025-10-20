use crate::v2::kuadrant::ReqRespCtx;

pub struct Pipeline {
    ctx: ReqRespCtx,
}

impl Pipeline {
    pub fn new(ctx: ReqRespCtx) -> Self {
        Self { ctx }
    }

    pub fn eval(mut self) -> Option<Self> {
        todo!()
    }
}
