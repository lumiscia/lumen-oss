use std::{collections::HashMap, ops::Range, sync::Arc};

use crate::{
    error::RenderError,
    media::MediaStore,
    node::{NodeId, PortRef},
    raster::{BitmapFrame, RasterFrame},
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
        ctx: &RenderContext<'_, S, M>,
    ) -> crate::Result<RasterFrame> {
        let w = ctx.renderer.composition.render_settings.width;
        let h = ctx.renderer.composition.render_settings.height;
        let pixel_count = w
            .checked_mul(h)
            .and_then(|count| count.checked_mul(4))
            .ok_or(RenderError::SurfaceAllocation {
                width: w,
                height: h,
            })?;

        Ok(RasterFrame::Bitmap(BitmapFrame::new(
            Arc::new(vec![0; pixel_count as usize]),
            w,
            h,
        )))
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
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<RasterFrame> {
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

        let value = ctx.eval(layer.clone())?;
        value.as_raster()?.snapshot()
    }
}
