pub mod context;
pub mod stats;
pub mod surface;
#[cfg(feature = "threading")]
pub mod threading;

pub use context::RenderContext;

use std::rc::Rc;

use crate::{
    composition::Composition,
    error::GraphValidationError,
    media::MediaStore,
    node::{NodeResult, PortRef},
    raster::RasterFrame,
    render::surface::SurfacePool,
};

#[derive(Debug)]
pub struct LumenRenderer<'a, S: SurfacePool, M: MediaStore> {
    pub composition: &'a Composition,

    pub surface_pool: &'a S,
    pub media_store: &'a M,
}

impl<'a, S: SurfacePool, M: MediaStore> LumenRenderer<'a, S, M> {
    pub fn new(
        composition: &'a Composition,
        surface_pool: &'a S,
        media_store: &'a M,
    ) -> crate::Result<Self> {
        Ok(Self {
            composition,
            surface_pool,
            media_store,
        })
    }

    pub fn render(&mut self, frame: u32) -> crate::Result<RasterFrame> {
        let mut media_outputs =
            self.composition
                .graph
                .nodes
                .iter()
                .filter_map(|(node_id, node)| {
                    matches!(node, crate::node::NodeKind::MediaOutput(_)).then_some(*node_id)
                });
        let Some(output_node_id) = media_outputs.next() else {
            return Err(GraphValidationError::MissingMediaOutput.into());
        };
        if let Some(_) = media_outputs.next() {
            return Err(GraphValidationError::MultipleMediaOutputs { count: 2 }.into());
        }

        let mut ctx = RenderContext::new(self, frame);
        let output = ctx.eval(PortRef::new(output_node_id, "output".to_string()))?;
        let raster = match Rc::try_unwrap(output) {
            Ok(NodeResult::Raster(raster)) => raster,
            Ok(NodeResult::Vector(_)) => {
                return Err(ctx.invalid_node_output_type(output_node_id, "RasterFrame", "Vector"));
            }
            Ok(NodeResult::None) => return Err(ctx.missing_node_output_error(output_node_id)),
            Err(shared) => match shared.as_ref() {
                NodeResult::Raster(raster) => raster.snapshot()?,
                NodeResult::Vector(_) => {
                    return Err(ctx.invalid_node_output_type(
                        output_node_id,
                        "RasterFrame",
                        "Vector",
                    ));
                }
                NodeResult::None => return Err(ctx.missing_node_output_error(output_node_id)),
            },
        };
        Ok(raster)
    }
}
