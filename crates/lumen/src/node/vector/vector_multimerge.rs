use crate::{
    node::{NodeId, PortRef, VectorData},
    render::RenderContext,
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Clone, Node)]
pub struct VectorMultiMerge {
    pub id: NodeId,

    #[input(kind = Vector, variadic)]
    pub layers: Vec<PortRef>,
}

impl Default for VectorMultiMerge {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            layers: Vec::new(),
        }
    }
}

#[node_impl]
impl VectorMultiMerge {
    #[output(port = "output", kind = Vector)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<VectorData> {
        let mut merged = Vec::new();
        for layer in &self.layers {
            if !layer.is_empty() {
                let result = ctx.eval(layer.clone())?;
                merged.push(result.as_vector()?.clone());
            }
        }

        let output = match merged.len() {
            0 => VectorData::Group {
                children: Vec::new(),
                position: Default::default(),
            },
            1 => merged.pop().expect("length checked"),
            _ => VectorData::Group {
                children: merged,
                position: Default::default(),
            },
        };

        Ok(output)
    }
}
