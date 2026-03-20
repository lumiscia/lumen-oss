use skia_safe::{Paint, image_filters};

use crate::{
    node::{
        NodeId, NodeProperty, PortRef,
        pixel_utils::{ClearMode, render_to_surface_ephemeral},
    },
    raster::RasterFrame,
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
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<RasterFrame> {
        let radius = self.resolve_radius(ctx)? as f32;
        let source_result = ctx.eval(self.source.clone())?;
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
        render_to_surface_ephemeral(
            width,
            height,
            ctx,
            source_format,
            source_data,
            source_alpha,
            ClearMode::Transparent,
            |canvas| {
                if let Some(blur_filter) = image_filters::blur((sigma, sigma), None, None, None) {
                    let mut paint = Paint::default();
                    paint.set_image_filter(blur_filter);
                    canvas.draw_image(&image, (0.0, 0.0), Some(&paint));
                } else {
                    // Fallback if blur filter creation fails
                    canvas.draw_image(&image, (0.0, 0.0), None);
                }
            },
        )
    }
}
