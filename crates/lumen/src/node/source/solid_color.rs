use crate::{
    gpu_image::{AlphaMode, GpuImageFrame, RectI},
    node::{
        NodeId, NodeProperty,
        pixel_utils::{ClearMode, render_to_surface_ephemeral, rgba_byte_len, to_skia_color},
    },
    render::RenderContext,
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Clone, Node)]
pub struct SolidColor {
    pub id: NodeId,

    #[property(expected = Color)]
    pub color: NodeProperty,
    #[property(expected = Int)]
    pub width: NodeProperty,
    #[property(expected = Int)]
    pub height: NodeProperty,
}

impl Default for SolidColor {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            color: NodeProperty::Color([0, 0, 0, 255]),
            width: NodeProperty::Int(0),
            height: NodeProperty::Int(0),
        }
    }
}

#[node_impl]
impl SolidColor {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<GpuImageFrame> {
        let requested_width = self.resolve_width(ctx)?;
        let requested_height = self.resolve_height(ctx)?;

        let width = if requested_width <= 0 {
            i64::from(ctx.renderer.composition.render_settings.width)
        } else {
            requested_width
        };
        let height = if requested_height <= 0 {
            i64::from(ctx.renderer.composition.render_settings.height)
        } else {
            requested_height
        };

        let mut width = width.clamp(1, i64::from(u32::MAX)) as u32;
        let mut height = height.clamp(1, i64::from(u32::MAX)) as u32;
        if rgba_byte_len(width, height).is_none() {
            width = 1;
            height = 1;
        }

        let color = to_skia_color(self.resolve_color(ctx)?);
        let rect = RectI::from_size(width, height);
        render_to_surface_ephemeral(
            width,
            height,
            ctx,
            rect,
            rect,
            AlphaMode::Premultiplied,
            ClearMode::None,
            |canvas| {
                canvas.clear(color);
            },
        )
    }
}
