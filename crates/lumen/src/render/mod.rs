pub mod context;
pub mod stats;
pub mod surface;
#[cfg(feature = "threading")]
pub mod threading;

pub use context::RenderContext;

use std::rc::Rc;

use crate::{
    composition::Composition,
    media::MediaStore,
    node::{NodeResult, PortRef},
    raster::BitmapFrame,
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

    // todo: proper error handling
    pub fn render(&mut self, frame: u32) -> crate::Result<BitmapFrame> {
        let output_node_id = self
            .composition
            .graph
            .nodes
            .iter()
            .find_map(|node| {
                if matches!(node.1, crate::node::NodeKind::MediaOutput(_)) {
                    Some(*node.0)
                } else {
                    None
                }
            })
            .ok_or_else(|| todo!("new error type"))
            .unwrap();

        let mut ctx = RenderContext::new(self, frame);
        let output =
            Rc::try_unwrap(ctx.eval(PortRef::new(output_node_id, "output".to_string()))?).unwrap();

        let raster = match output {
            NodeResult::Raster(raster) => raster.into_bitmap_frame(),
            NodeResult::Vector(_) => unreachable!(),
            NodeResult::None => unreachable!(),
        }
        .unwrap();
        // create ctx then call request_node_output on it
        Ok(raster)
    }
}
