use std::sync::Arc;

use crate::{
    node::{
        NodeId, NodeResult, PortRef,
        compositing::merge::draw_frame_image,
        pixel_utils::{make_skia_image, render_with_skia},
    },
    raster::{BitmapFrame, RasterFrame, RectI},
    render::RenderContext,
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Clone, Node)]
pub struct MediaOutput {
    pub id: NodeId,

    #[input(kind = Raster)]
    pub source: PortRef,
}

impl Default for MediaOutput {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            source: PortRef::empty(),
        }
    }
}

#[node_impl]
impl MediaOutput {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext<'_>) -> crate::Result<RasterFrame> {
        let source = match ctx.eval_once(self.source.clone())? {
            NodeResult::Raster(raster) => raster,
            NodeResult::Vector(_) => {
                return Err(ctx.invalid_node_output_type(self.source.id, "RasterFrame", "Vector"));
            }
            NodeResult::None => return Err(ctx.missing_node_output_error(self.source.id)),
        };

        let output_rect = RectI::from_size(
            ctx.renderer.composition.render_settings.width,
            ctx.renderer.composition.render_settings.height,
        );
        let (target_w, target_h) = (output_rect.width, output_rect.height);
        let (source_w, source_h) = source.dimensions();
        let source_format = source.format_rect();
        let source_data = source.data_rect();

        if source_w == target_w
            && source_h == target_h
            && source_format == output_rect
            && source_data == output_rect
        {
            return source.into_bitmap_frame().map(RasterFrame::Bitmap);
        }

        if target_w == 0 || target_h == 0 {
            return Ok(RasterFrame::Bitmap(BitmapFrame::with_domain(
                Arc::new(Vec::new()),
                0,
                0,
                output_rect,
                output_rect,
            )));
        }

        let source_alpha = source.alpha_mode();
        let (source_bytes, storage_w, storage_h) = source.into_parts();
        let Some(image) = make_skia_image(
            source_bytes.as_slice(),
            storage_w,
            storage_h,
            (storage_w as usize) * 4,
            source_alpha,
        ) else {
            return Ok(RasterFrame::Bitmap(
                BitmapFrame::with_domain(
                    Arc::new(vec![0; (target_w as usize) * (target_h as usize) * 4]),
                    target_w,
                    target_h,
                    output_rect,
                    output_rect,
                )
                .with_alpha_mode(source_alpha),
            ));
        };

        let output = render_with_skia(target_w, target_h, Some(ctx), |canvas| {
            draw_frame_image(
                canvas,
                &image,
                storage_w,
                storage_h,
                source_format,
                source_data,
                output_rect,
                None,
            );
        });

        Ok(RasterFrame::Bitmap(
            BitmapFrame::with_domain(
                Arc::new(output),
                target_w,
                target_h,
                output_rect,
                output_rect,
            )
            .with_alpha_mode(source_alpha),
        ))
    }
}
