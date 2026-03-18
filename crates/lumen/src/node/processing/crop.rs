use std::sync::Arc;

use skia_safe::{IRect, image::RequiredProperties};

use crate::{
    error::LumenError,
    node::{NodeId, NodeProperty, PortRef, pixel_utils::make_skia_image},
    raster::{BitmapFrame, RasterFrame, RectI},
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
        let source = ctx.eval(self.source.clone())?.as_raster()?;
        let x = self.resolve_x(ctx)?;
        let y = self.resolve_y(ctx)?;
        let width = self.resolve_width(ctx)?.max(0);
        let height = self.resolve_height(ctx)?.max(0);

        let (src_w, src_h) = source.dimensions();
        let source_alpha = source.alpha_mode();
        let source_format = source.format_rect();

        // Calculate crop boundaries clamped to source dimensions
        let crop_left = x.clamp(0, src_w as i64) as i32;
        let crop_top = y.clamp(0, src_h as i64) as i32;
        let crop_right = (x + width).clamp(0, src_w as i64) as i32;
        let crop_bottom = (y + height).clamp(0, src_h as i64) as i32;

        // Check if crop covers entire source image (no-op case)
        if crop_left == 0
            && crop_top == 0
            && crop_right == src_w as i32
            && crop_bottom == src_h as i32
            && crop_right > crop_left
            && crop_bottom > crop_top
        {
            return Ok(source.clone());
        }

        // Return 1x1 transparent pixel if crop area is empty
        if crop_right <= crop_left || crop_bottom <= crop_top {
            let transparent_pixel = Arc::new(vec![0, 0, 0, 0]);
            let output_rect = RectI::new(
                source_format.x + crop_left,
                source_format.y + crop_top,
                1,
                1,
            );
            return Ok(RasterFrame::Bitmap(
                BitmapFrame::with_domain(transparent_pixel, 1, 1, output_rect, output_rect)
                    .with_alpha_mode(source_alpha),
            ));
        }

        let (bytes, src_w, src_h) = source.clone().into_parts();
        let Some(image) = make_skia_image(&bytes, src_w, src_h, (src_w as usize) * 4, source_alpha)
        else {
            // Failed to create Skia image, return transparent pixel
            let transparent_pixel = Arc::new(vec![0, 0, 0, 0]);
            let output_rect = RectI::new(
                source_format.x + crop_left,
                source_format.y + crop_top,
                1,
                1,
            );
            return Ok(RasterFrame::Bitmap(
                BitmapFrame::with_domain(transparent_pixel, 1, 1, output_rect, output_rect)
                    .with_alpha_mode(source_alpha),
            ));
        };

        let subset_rect = IRect::from_ltrb(crop_left, crop_top, crop_right, crop_bottom);
        let crop_width = (crop_right - crop_left) as u32;
        let crop_height = (crop_bottom - crop_top) as u32;
        let output_rect = RectI::new(
            source_format.x + crop_left,
            source_format.y + crop_top,
            crop_width,
            crop_height,
        );
        // Try to create subset image directly from source
        if let Some(cropped_image) =
            image.make_subset(None, &subset_rect, RequiredProperties::default())
        {
            if let Some(pixel_data) = cropped_image.peek_pixels() {
                if let Some(pixel_bytes) = pixel_data.bytes() {
                    return Ok(RasterFrame::Bitmap(
                        BitmapFrame::with_domain(
                            Arc::new(pixel_bytes.to_vec()),
                            crop_width,
                            crop_height,
                            output_rect,
                            output_rect,
                        )
                        .with_alpha_mode(source_alpha),
                    ));
                }
            }
        }

        // Fallback: render crop using canvas
        let output = crate::node::pixel_utils::render_with_skia(
            crop_width,
            crop_height,
            Some(ctx),
            |canvas| {
                canvas.draw_image(&image, (-crop_left as f32, -crop_top as f32), None);
            },
        );

        Ok(RasterFrame::Bitmap(
            BitmapFrame::with_domain(
                Arc::new(output),
                crop_width,
                crop_height,
                output_rect,
                output_rect,
            )
            .with_alpha_mode(source_alpha),
        ))
    }
}
