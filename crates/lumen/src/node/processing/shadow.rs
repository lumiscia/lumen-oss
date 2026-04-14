use skia_safe::{Paint, image_filters};

use crate::{
    node::{
        NodeId, NodeProperty, PortRef,
        compositing::merge::union_rect,
        pixel_utils::{ClearMode, render_to_surface_ephemeral, to_skia_color},
        processing::filter_geometry::{expand_rect, filter_pad, offset_rect},
    },
    raster::{RasterFrame, RectI},
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
        let source_result = ctx.eval(&self.source)?;
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
        let (image, width, height) = match source.image_parts() {
            Some(parts) => parts,
            None => return source.snapshot(),
        };

        if width == 0 || height == 0 {
            return source.snapshot();
        }

        let shadow_color = to_skia_color(color);
        // Ensure non-negative blur radius
        let sigma = blur_radius.max(0.0);

        let shadow_pad = filter_pad(sigma);
        let shadow_format = offset_rect(source_format, offset_x as i32, offset_y as i32);
        let shadow_data = offset_rect(source_data, offset_x as i32, offset_y as i32);
        let output_format = union_rect(source_format, expand_rect(shadow_format, shadow_pad));
        let output_data = union_rect(source_data, expand_rect(shadow_data, shadow_pad))
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
                let draw_origin = (
                    (source_format.x - output_format.x) as f32,
                    (source_format.y - output_format.y) as f32,
                );
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
                    canvas.draw_image(&image, draw_origin, Some(&paint_with_shadow));
                }

                // Draw original image on top of shadow
                canvas.draw_image(&image, draw_origin, None);
            },
        )
    }
}
