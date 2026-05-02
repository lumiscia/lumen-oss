use skia_safe::{Paint, image_filters};

use crate::{
    gpu_image::{GpuImageFrame, RectI},
    node::{
        NodeId, NodeProperty, PortRef,
        pixel_utils::{ClearMode, render_to_surface_ephemeral},
        processing::filter_geometry::{expand_rect, filter_pad},
    },
    render::RenderContext,
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Clone, Node)]
pub struct Blur {
    pub id: NodeId,

    #[property(expected = Float)]
    pub radius: NodeProperty,

    #[input(kind = Raster)]
    pub source: PortRef,
}

impl Default for Blur {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            radius: NodeProperty::Float(0.0),
            source: PortRef::empty(),
        }
    }
}

impl Blur {
    /// Returns true if the blur operation would have no visual effect
    pub fn is_noop(radius: f32) -> bool {
        radius <= 0.0
    }
}
#[node_impl]
impl Blur {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<GpuImageFrame> {
        let radius = self.resolve_radius(ctx)? as f32;
        let source_result = ctx.eval(&self.source)?;
        let source = source_result.as_raster()?;

        if Self::is_noop(radius) {
            return source.snapshot();
        }

        let source_alpha = source.alpha_mode();
        let source_format = source.format_rect();
        let source_data = source.data_rect();
        let (image, width, height) = match source.image_parts() {
            Some(parts) => parts,
            None => return source.snapshot(),
        };

        if width == 0 || height == 0 {
            return source.snapshot();
        }

        // Use minimum sigma of 0.5 to ensure visible blur effect
        let sigma = radius.max(0.5);
        let pad = filter_pad(sigma);
        let output_format = expand_rect(source_format, pad);
        let output_data = expand_rect(source_data, pad)
            .intersect(&output_format)
            .unwrap_or(RectI::new(output_format.x, output_format.y, 0, 0));

        render_to_surface_ephemeral(
            output_format.width.max(width).max(1),
            output_format.height.max(height).max(1),
            ctx,
            output_format,
            output_data,
            source_alpha,
            ClearMode::Transparent,
            |canvas| {
                let offset_x = (source_format.x - output_format.x) as f32;
                let offset_y = (source_format.y - output_format.y) as f32;
                if let Some(blur_filter) = image_filters::blur((sigma, sigma), None, None, None) {
                    let mut paint = Paint::default();
                    paint.set_image_filter(blur_filter);
                    canvas.draw_image(&image, (offset_x, offset_y), Some(&paint));
                } else {
                    // Fallback if blur filter creation fails
                    canvas.draw_image(&image, (offset_x, offset_y), None);
                }
            },
        )
    }
}
