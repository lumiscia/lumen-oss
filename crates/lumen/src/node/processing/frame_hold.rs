use crate::{
    node::{NodeId, NodeProperty, PortRef},
    raster::RasterFrame,
    render::RenderContext,
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Clone, Node)]
pub struct FrameHold {
    pub id: NodeId,

    #[property(expected = Int)]
    pub hold_frame: NodeProperty,

    #[input(kind = Raster)]
    pub source: PortRef,
}

impl Default for FrameHold {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            hold_frame: NodeProperty::Int(0),
            source: PortRef::empty(),
        }
    }
}

#[node_impl]
impl FrameHold {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<RasterFrame> {
        let hold_frame = self.resolve_hold_frame(ctx)? as u32;
        let original_frame = ctx.frame;
        ctx.frame = hold_frame;
        let result = ctx.eval(&self.source)?.as_raster()?.snapshot();
        ctx.frame = original_frame;
        result
    }
}
