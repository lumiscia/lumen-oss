use std::sync::Arc;

use skia_safe::Paint;

use crate::{
    node::{
        NodeId, NodeProperty, PortRef,
        pixel_utils::{make_skia_image, render_with_skia},
        vector::shape_renderer::{ShapeRenderer, rasterize_vector},
    },
    raster::{AlphaMode, BitmapFrame, RasterFrame},
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
        let source = ctx.eval(self.source.clone())?;
        let source = source.as_raster()?;
        let (source_bytes, source_w, source_h) = source.snapshot_parts()?;
        let source_alpha = source.alpha_mode();
        let source_format = source.format_rect();
        let source_data = source.data_rect();

        let mask = if !self.mask.is_empty() {
            let frame = ctx.eval(self.mask.clone())?;
            Some(frame.as_raster()?.snapshot_parts()?)
        } else if !self.vector.is_empty() {
            let vector = ctx.eval(self.vector.clone())?;
            Some(rasterize_vector(vector.as_vector()?, &ShapeRenderer::default(), ctx).into_parts())
        } else {
            None
        };

        let Some((mask_bytes, mask_w, mask_h)) = mask else {
            return source.snapshot();
        };

        let out_w = source_w;
        let out_h = source_h;
        if out_w == 0 || out_h == 0 {
            return Ok(RasterFrame::Bitmap(
                BitmapFrame::with_domain(Arc::new(Vec::new()), 0, 0, source_format, source_data)
                    .with_alpha_mode(source_alpha),
            ));
        }

        let source_image = make_skia_image(
            &source_bytes,
            source_w,
            source_h,
            (source_w as usize) * 4,
            source_alpha,
        );
        let mask_image = make_skia_image(
            &mask_bytes,
            mask_w,
            mask_h,
            (mask_w as usize) * 4,
            AlphaMode::Premultiplied,
        );

        let (source_image, mask_image) = match (source_image, mask_image) {
            (Some(source_image), Some(mask_image)) => (source_image, mask_image),
            _ => {
                return Ok(RasterFrame::Bitmap(
                    BitmapFrame::with_domain(
                        Arc::new(vec![0u8; (out_w as usize) * (out_h as usize) * 4]),
                        out_w,
                        out_h,
                        source_format,
                        source_data,
                    )
                    .with_alpha_mode(source_alpha),
                ));
            }
        };

        let mask_kind = MaskKind::from_int(self.resolve_mask_kind(ctx)?);
        let invert = self.resolve_invert(ctx)?;

        let output = render_with_skia(out_w, out_h, Some(ctx), |canvas| {
            canvas.draw_image(&source_image, (0.0, 0.0), None);

            // For luma masking, we need to convert the mask to a grayscale alpha.
            // For alpha masking, we use the mask's alpha channel directly.
            // In both cases, DstIn keeps destination where mask is opaque,
            // DstOut keeps destination where mask is transparent.
            let blend_mode = if invert {
                skia_safe::BlendMode::DstOut
            } else {
                skia_safe::BlendMode::DstIn
            };

            if mask_kind == MaskKind::Luma {
                // For luma masking, draw a luminance-based alpha mask.
                // We create a layer, draw the mask, then apply a color filter
                // that converts RGB to alpha based on luminance.
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
        });

        Ok(RasterFrame::Bitmap(
            BitmapFrame::with_domain(Arc::new(output), out_w, out_h, source_format, source_data)
                .with_alpha_mode(source_alpha),
        ))
    }
}
