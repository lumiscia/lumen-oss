use skia_safe::{Paint, canvas::SaveLayerRec};

use crate::{
    node::{
        NodeId, NodeProperty, PortRef,
        compositing::merge::draw_frame_image,
        pixel_utils::{ClearMode, render_to_surface_ephemeral},
        vector::shape_renderer::{ShapeRenderer, rasterize_vector},
    },
    raster::RasterFrame,
    render::RenderContext,
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
#[non_exhaustive]
pub enum MaskKind {
    Alpha = 0,
    Luma = 1,
}

impl MaskKind {
    fn from_int(value: i64) -> Self {
        match value {
            1 => Self::Luma,
            _ => Self::Alpha,
        }
    }
}

#[derive(Debug, Clone, Node)]
pub struct Boolean {
    pub id: NodeId,

    #[property(expected = Int)]
    pub mask_kind: NodeProperty,
    #[property(expected = Bool)]
    pub invert: NodeProperty,

    #[input(kind = Raster)]
    pub source: PortRef,
    #[input(kind = Raster, optional)]
    pub mask: PortRef,
    #[input(kind = Vector, optional)]
    pub vector: PortRef,
}

impl Default for Boolean {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            mask_kind: NodeProperty::Int(MaskKind::Alpha as i64),
            invert: NodeProperty::Bool(false),
            source: PortRef::empty(),
            mask: PortRef::empty(),
            vector: PortRef::empty(),
        }
    }
}

#[node_impl]
impl Boolean {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<RasterFrame> {
        let source_result = ctx.eval(&self.source)?;
        let source = source_result.as_raster()?;
        let source_alpha = source.alpha_mode();
        let source_format = source.format_rect();
        let source_data = source.data_rect();

        let (source_image, source_w, source_h) = match source.image_parts() {
            Some(parts) => parts,
            None => return source.snapshot(),
        };

        let mask_frame = if !self.mask.is_empty() {
            let frame = ctx.eval(&self.mask)?;
            Some(frame.as_raster()?.snapshot()?)
        } else if !self.vector.is_empty() {
            let vector = ctx.eval(&self.vector)?;
            Some(rasterize_vector(
                vector.as_vector()?,
                &ShapeRenderer::default(),
                ctx,
            ))
        } else {
            None
        };

        let Some(mask_frame) = mask_frame else {
            return source.snapshot();
        };
        let Some((mask_image, mask_w, mask_h)) = mask_frame.image_parts() else {
            return source.snapshot();
        };
        let mask_format = mask_frame.format_rect();
        let mask_data = mask_frame.data_rect();

        let out_w = source_w;
        let out_h = source_h;
        if out_w == 0 || out_h == 0 {
            return render_to_surface_ephemeral(
                out_w.max(1),
                out_h.max(1),
                ctx,
                source_format,
                source_data,
                source_alpha,
                ClearMode::Transparent,
                |_| {},
            );
        }

        let mask_kind = MaskKind::from_int(self.resolve_mask_kind(ctx)?);
        let invert = self.resolve_invert(ctx)?;

        render_to_surface_ephemeral(
            out_w,
            out_h,
            ctx,
            source_format,
            source_data,
            source_alpha,
            ClearMode::Transparent,
            |canvas| {
                draw_frame_image(
                    canvas,
                    &source_image,
                    source_w,
                    source_h,
                    source_format,
                    source_data,
                    source_format,
                    None,
                );

                let blend_mode = if invert {
                    skia_safe::BlendMode::DstOut
                } else {
                    skia_safe::BlendMode::DstIn
                };
                let mut layer_paint = Paint::default();
                layer_paint.set_blend_mode(blend_mode);
                canvas.save_layer(&SaveLayerRec::default().paint(&layer_paint));

                if mask_kind == MaskKind::Luma {
                    let luma_cf = skia_safe::ColorFilter::luma();
                    let mut mask_paint = Paint::default();
                    mask_paint.set_color_filter(luma_cf);
                    draw_frame_image(
                        canvas,
                        &mask_image,
                        mask_w,
                        mask_h,
                        mask_format,
                        mask_data,
                        source_format,
                        Some(&mask_paint),
                    );
                } else {
                    draw_frame_image(
                        canvas,
                        &mask_image,
                        mask_w,
                        mask_h,
                        mask_format,
                        mask_data,
                        source_format,
                        None,
                    );
                }
                canvas.restore();
            },
        )
    }
}
