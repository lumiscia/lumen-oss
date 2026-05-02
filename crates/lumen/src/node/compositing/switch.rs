use std::{collections::HashMap, ops::Range};

use crate::{
    error::RenderError,
    gpu_image::{AlphaMode, GpuImageFrame, RectI},
    media::MediaStore,
    node::{
        NodeId, PortRef,
        pixel_utils::{ClearMode, render_to_surface_ephemeral},
    },
    render::{RenderContext, surface::SurfacePool},
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Clone, Node)]
pub struct Switch {
    pub id: NodeId,
    pub map: HashMap<u16, Range<u32>>,

    #[input(kind = Raster, optional, variadic)]
    pub layers: Vec<PortRef>,
}

impl Switch {
    pub fn new(map: HashMap<u16, Range<u32>>) -> Self {
        Self {
            map,
            ..Self::default()
        }
    }

    fn transparent_output<S: SurfacePool, M: MediaStore>(
        ctx: &mut RenderContext<'_, S, M>,
    ) -> crate::Result<GpuImageFrame> {
        let w = ctx.renderer.composition.render_settings.width;
        let h = ctx.renderer.composition.render_settings.height;
        let pixel_count = w.checked_mul(h).ok_or(RenderError::SurfaceAllocation {
            width: w,
            height: h,
        })?;
        let _ = pixel_count;
        render_to_surface_ephemeral(
            w,
            h,
            ctx,
            RectI::from_size(w, h),
            RectI::from_size(w, h),
            AlphaMode::Premultiplied,
            ClearMode::Transparent,
            |_| {},
        )
    }
}

impl Default for Switch {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            map: HashMap::new(),
            layers: Vec::new(),
        }
    }
}

#[node_impl]
impl Switch {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<GpuImageFrame> {
        let selected_index = self
            .map
            .iter()
            .filter_map(|(index, frame_range)| frame_range.contains(&ctx.frame).then_some(*index))
            .min();

        let Some(index) = selected_index else {
            return Self::transparent_output(ctx);
        };

        let Some(layer) = self.layers.get(index as usize) else {
            return Self::transparent_output(ctx);
        };

        if layer.is_empty() {
            return Self::transparent_output(ctx);
        }

        let value = ctx.eval(layer)?;
        value.as_raster()?.snapshot()
    }
}
