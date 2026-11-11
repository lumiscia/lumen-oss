//! How the frame graph records the nodes that output a scene. `render_3d` reads its scene without
//! recording it, and validation keeps a scene out of every raster input, so recording one fails.
//! Each builds the programs that drawing it adds to its scene's.

#[cfg(not(feature = "3d"))]
use super::Render3d;
#[cfg(feature = "3d")]
use super::render::{catcher_programs, light_programs};
use super::{Camera3d, ImagePlane3d, Instance3d, Light3d, Merge3d, Model3d, Shape3d, Text3d};
use crate::{
    error::{LumenError, RenderError},
    gpu::frame_graph::{FrameCx, Programs, Raster, RecordNode},
    node::NodeId,
};

fn not_a_raster(node_id: NodeId) -> LumenError {
    RenderError::NodeEvaluation {
        node_id,
        node_kind: "Scene",
        details: "a scene node outputs a scene, not a raster; draw it through a render_3d"
            .to_string(),
    }
    .into()
}

/// Without the `3d` feature nothing draws a scene, so no light builds programs.
#[cfg(not(feature = "3d"))]
fn light_programs(_: &Light3d, _: &mut Programs<'_>) -> crate::Result<()> {
    Ok(())
}

/// Without the `3d` feature nothing draws a scene, so no shadow catcher builds programs.
#[cfg(not(feature = "3d"))]
fn catcher_programs(_: &crate::node::Deferred<bool>, _: &mut Programs<'_>) -> crate::Result<()> {
    Ok(())
}

impl RecordNode for Shape3d {
    fn programs(&self, programs: &mut Programs<'_>) -> crate::Result<()> {
        catcher_programs(&self.params.material.shadow_catcher, programs)
    }

    fn record(&self, _: &mut FrameCx<'_>) -> crate::Result<Raster> {
        Err(not_a_raster(self.id))
    }
}

impl RecordNode for ImagePlane3d {
    fn programs(&self, programs: &mut Programs<'_>) -> crate::Result<()> {
        catcher_programs(&self.params.material.shadow_catcher, programs)
    }

    fn record(&self, _: &mut FrameCx<'_>) -> crate::Result<Raster> {
        Err(not_a_raster(self.id))
    }
}

impl RecordNode for Text3d {
    fn programs(&self, programs: &mut Programs<'_>) -> crate::Result<()> {
        catcher_programs(&self.params.material.shadow_catcher, programs)
    }

    fn record(&self, _: &mut FrameCx<'_>) -> crate::Result<Raster> {
        Err(not_a_raster(self.id))
    }
}

impl RecordNode for Light3d {
    fn programs(&self, programs: &mut Programs<'_>) -> crate::Result<()> {
        light_programs(self, programs)
    }

    fn record(&self, _: &mut FrameCx<'_>) -> crate::Result<Raster> {
        Err(not_a_raster(self.id))
    }
}

impl RecordNode for Model3d {
    fn programs(&self, _: &mut Programs<'_>) -> crate::Result<()> {
        Ok(())
    }

    fn record(&self, _: &mut FrameCx<'_>) -> crate::Result<Raster> {
        Err(not_a_raster(self.id))
    }
}

impl RecordNode for Camera3d {
    fn programs(&self, _: &mut Programs<'_>) -> crate::Result<()> {
        Ok(())
    }

    fn record(&self, _: &mut FrameCx<'_>) -> crate::Result<Raster> {
        Err(not_a_raster(self.id))
    }
}

impl RecordNode for Merge3d {
    fn programs(&self, _: &mut Programs<'_>) -> crate::Result<()> {
        Ok(())
    }

    fn record(&self, _: &mut FrameCx<'_>) -> crate::Result<Raster> {
        Err(not_a_raster(self.id))
    }
}

impl RecordNode for Instance3d {
    fn programs(&self, _: &mut Programs<'_>) -> crate::Result<()> {
        Ok(())
    }

    fn record(&self, _: &mut FrameCx<'_>) -> crate::Result<Raster> {
        Err(not_a_raster(self.id))
    }
}

/// Without the `3d` feature a `render_3d` fails to record, and builds no programs.
#[cfg(not(feature = "3d"))]
impl RecordNode for Render3d {
    fn programs(&self, _: &mut Programs<'_>) -> crate::Result<()> {
        Ok(())
    }

    fn record(&self, _: &mut FrameCx<'_>) -> crate::Result<Raster> {
        Err(RenderError::NodeEvaluation {
            node_id: self.id,
            node_kind: "Render3d",
            details: crate::error::NO_3D_SUPPORT.to_string(),
        }
        .into())
    }
}
