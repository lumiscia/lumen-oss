use std::sync::Arc;

use skia_safe::{Paint, image_filters};

use crate::{
    node::{
        NodeId, NodeProperty, PortRef,
        pixel_utils::{make_skia_image, render_with_skia, to_skia_color},
    },
    raster::{BitmapFrame, RasterFrame},
    render::RenderContext,
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Clone, Node)]
pub struct Shadow {
    pub id: NodeId,

    #[property(expected = Int)]
    pub offset_x: NodeProperty,
    #[property(expected = Int)]
    pub offset_y: NodeProperty,
    #[property(expected = Color)]
    pub color: NodeProperty,
    #[property(expected = Float)]
    pub blur_radius: NodeProperty,

    #[input(kind = Raster)]
    pub source: PortRef,
}

impl Default for Shadow {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            offset_x: NodeProperty::Int(0),
            offset_y: NodeProperty::Int(0),
            color: NodeProperty::Color([0, 0, 0, 255]),
            blur_radius: NodeProperty::Float(0.0),
            source: PortRef::empty(),
        }
    }
}

#[node_impl]
impl Shadow {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<RasterFrame> {
        let source_result = ctx.eval(self.source.clone())?;
        let source = source_result.as_raster()?;
        let color = self.resolve_color(ctx)?;

        // Early return if shadow is fully transparent.

        if color[3] == 0 {
            return source.snapshot();
        }

        let offset_x = self.resolve_offset_x(ctx)? as f32;
        let offset_y = self.resolve_offset_y(ctx)? as f32;
        let blur_radius = self.resolve_blur_radius(ctx)? as f32;

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

        let shadow_color = to_skia_color(color);
        // Ensure non-negative blur radius
        let sigma = blur_radius.max(0.0);

        let output = render_with_skia(width, height, Some(ctx), |canvas| {
            // Create and apply drop shadow filter
            let shadow_filter = image_filters::drop_shadow_only(
                (offset_x, offset_y),
                (sigma, sigma),
                shadow_color,
                None,
                None,
                None,
            );

            if let Some(filter) = shadow_filter {
                let mut paint_with_shadow = Paint::default();
                paint_with_shadow.set_image_filter(filter);
                canvas.draw_image(&image, (0.0, 0.0), Some(&paint_with_shadow));
            }

            // Draw original image on top of shadow
            canvas.draw_image(&image, (0.0, 0.0), None);
        });

        Ok(RasterFrame::Bitmap(
            BitmapFrame::with_domain(Arc::new(output), width, height, source_format, source_data)
                .with_alpha_mode(source_alpha),
        ))
    }
}
