use crate::{
    error::LumenError,
    node::{NodeId, PortRef, VectorData},
    render::RenderContext,
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Clone, Node)]
pub struct VectorMerge {
    pub id: NodeId,

    #[input(kind = Vector)]
    pub base: PortRef,
    #[input(kind = Vector)]
    pub overlay: PortRef,
}

impl Default for VectorMerge {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            base: PortRef::empty(),
            overlay: PortRef::empty(),
        }
    }
}

#[node_impl]
impl VectorMerge {
    #[output(port = "output", kind = Vector)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<VectorData> {
        let base = ctx.eval(self.base.clone())?.as_vector()?.clone();
        let overlay = ctx.eval(self.overlay.clone())?.as_vector()?.clone();

        Ok(VectorData::Group {
            children: vec![base, overlay],
            position: Default::default(),
        })
    }
}
