use std::sync::Arc;

use skia_safe::{Paint, image_filters};

use crate::{
    node::{
        NodeId, NodeProperty, PortRef,
        pixel_utils::{make_skia_image, render_with_skia},
    },
    raster::{BitmapFrame, RasterFrame},
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
        let (bytes, width, height) = source.snapshot_parts()?;

        if width == 0 || height == 0 {
            return Ok(RasterFrame::Bitmap(
                BitmapFrame::with_domain(bytes, width, height, source_format, source_data)
                    .with_alpha_mode(source_alpha),
            ));
        }

        let Some(image) =
            make_skia_image(&bytes, width, height, (width as usize) * 4, source_alpha)
        else {
            return Ok(RasterFrame::Bitmap(
                BitmapFrame::with_domain(bytes, width, height, source_format, source_data)
                    .with_alpha_mode(source_alpha),
            ));
        };

        // Use minimum sigma of 0.5 to ensure visible blur effect
        let sigma = radius.max(0.5);
        let blurred = render_with_skia(width, height, Some(ctx), |canvas| {
            if let Some(blur_filter) = image_filters::blur((sigma, sigma), None, None, None) {
                let mut paint = Paint::default();
                paint.set_image_filter(blur_filter);
                canvas.draw_image(&image, (0.0, 0.0), Some(&paint));
            } else {
                // Fallback if blur filter creation fails
                canvas.draw_image(&image, (0.0, 0.0), None);
            }
        });

        Ok(RasterFrame::Bitmap(
            BitmapFrame::with_domain(Arc::new(blurred), width, height, source_format, source_data)
                .with_alpha_mode(source_alpha),
        ))
    }
}
