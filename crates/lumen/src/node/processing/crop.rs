use skia_safe::{IRect, image::RequiredProperties};

use crate::{
    node::{
        NodeId, NodeProperty, PortRef,
        pixel_utils::{ClearMode, render_to_surface_ephemeral},
    },
    raster::{ImageFrame, RasterFrame, RectI},
    render::RenderContext,
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Clone, Node)]
pub struct Crop {
    pub id: NodeId,

    #[property(expected = Int)]
    pub x: NodeProperty,
    #[property(expected = Int)]
    pub y: NodeProperty,
    #[property(expected = Int)]
    pub width: NodeProperty,
    #[property(expected = Int)]
    pub height: NodeProperty,

    #[input(kind = Raster)]
    pub source: PortRef,
}

impl Default for Crop {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            x: NodeProperty::Int(0),
            y: NodeProperty::Int(0),
            width: NodeProperty::Int(1),
            height: NodeProperty::Int(1),
            source: PortRef::empty(),
        }
    }
}

#[node_impl]
impl Crop {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<RasterFrame> {
        let source_result = ctx.eval(&self.source)?;
        let source = source_result.as_raster()?;
        let x = self.resolve_x(ctx)?;
        let y = self.resolve_y(ctx)?;
        let width = self.resolve_width(ctx)?.max(0);
        let height = self.resolve_height(ctx)?.max(0);

        let (src_w, src_h) = source.dimensions();
        let source_alpha = source.alpha_mode();
        let source_format = source.format_rect();

        let crop_left = x.clamp(0, src_w as i64) as i32;
        let crop_top = y.clamp(0, src_h as i64) as i32;
        let crop_right = (x + width).clamp(0, src_w as i64) as i32;
        let crop_bottom = (y + height).clamp(0, src_h as i64) as i32;

        if crop_left == 0
            && crop_top == 0
            && crop_right == src_w as i32
            && crop_bottom == src_h as i32
            && crop_right > crop_left
            && crop_bottom > crop_top
        {
            return source.snapshot();
        }

        if crop_right <= crop_left || crop_bottom <= crop_top {
            let output_rect = RectI::new(
                source_format.x + crop_left,
                source_format.y + crop_top,
                1,
                1,
            );
            return RasterFrame::transparent(1, 1, output_rect, output_rect, source_alpha);
        }

        let crop_width = (crop_right - crop_left) as u32;
        let crop_height = (crop_bottom - crop_top) as u32;
        let output_rect = RectI::new(
            source_format.x + crop_left,
            source_format.y + crop_top,
            crop_width,
            crop_height,
        );

        let image = match source.to_skia_image() {
            Some(img) => img,
            None => {
                return RasterFrame::transparent(1, 1, output_rect, output_rect, source_alpha);
            }
        };

        // Try fast subset path
        let subset_rect = IRect::from_ltrb(crop_left, crop_top, crop_right, crop_bottom);
        if let Some(cropped_image) =
            image.make_subset(None, &subset_rect, RequiredProperties::default())
        {
            let mut frame = ImageFrame::with_domain(
                cropped_image,
                crop_width,
                crop_height,
                output_rect,
                output_rect,
            );
            frame.alpha_mode = source_alpha;
            return Ok(frame);
        }

        // Fallback: render to surface
        render_to_surface_ephemeral(
            crop_width,
            crop_height,
            ctx,
            output_rect,
            output_rect,
            source_alpha,
            ClearMode::None,
            |canvas| {
                canvas.draw_image(&image, (-crop_left as f32, -crop_top as f32), None);
            },
        )
    }
}
