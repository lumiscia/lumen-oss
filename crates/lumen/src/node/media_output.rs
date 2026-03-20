use crate::{
    node::{
        NodeId, NodeResult, PortRef,
        compositing::merge::draw_frame_image,
        pixel_utils::{ClearMode, render_to_surface_stable},
    },
    raster::{AlphaMode, RasterFrame, RectI},
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
            return source.snapshot();
        }

        if target_w == 0 || target_h == 0 {
            return RasterFrame::transparent(
                0,
                0,
                output_rect,
                output_rect,
                AlphaMode::Premultiplied,
            );
        }

        let source_alpha = source.alpha_mode();
        let Some((image, storage_w, storage_h)) = source.image_parts() else {
            return RasterFrame::transparent(
                target_w,
                target_h,
                output_rect,
                output_rect,
                source_alpha,
            );
        };

        render_to_surface_stable(
            target_w,
            target_h,
            ctx,
            output_rect,
            output_rect,
            source_alpha,
            ClearMode::Transparent,
            |canvas| {
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
            },
        )
    }
}
