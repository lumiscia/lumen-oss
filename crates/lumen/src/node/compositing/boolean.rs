use skia_safe::Paint;

use crate::{
    node::{
        NodeId, NodeProperty, PortRef,
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

        let mask_image = if !self.mask.is_empty() {
            let frame = ctx.eval(&self.mask)?;
            frame.as_raster()?.to_skia_image()
        } else if !self.vector.is_empty() {
            let vector = ctx.eval(&self.vector)?;
            let rasterized = rasterize_vector(vector.as_vector()?, &ShapeRenderer::default(), ctx);
            rasterized.to_skia_image()
        } else {
            None
        };

        let Some(mask_image) = mask_image else {
            return source.snapshot();
        };

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
                canvas.draw_image(&source_image, (0.0, 0.0), None);

                let blend_mode = if invert {
                    skia_safe::BlendMode::DstOut
                } else {
                    skia_safe::BlendMode::DstIn
                };

                if mask_kind == MaskKind::Luma {
                    let luma_cf = skia_safe::ColorFilter::luma();
                    let mut mask_paint = Paint::default();
                    mask_paint.set_blend_mode(blend_mode);
                    mask_paint.set_color_filter(luma_cf);
                    canvas.draw_image(&mask_image, (0.0, 0.0), Some(&mask_paint));
                } else {
                    let mut mask_paint = Paint::default();
                    mask_paint.set_blend_mode(blend_mode);
                    canvas.draw_image(&mask_image, (0.0, 0.0), Some(&mask_paint));
                }
            },
        )
    }
}
