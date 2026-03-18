use std::sync::Arc;

use skia_safe::Paint;

use crate::{
    error::{LumenError, RenderError},
    media::MediaStore,
    node::{
        NodeId, NodeProperty, PortRef,
        compositing::{
            BlendMode,
            merge::{draw_frame_image, union_rect},
        },
        pixel_utils::{make_skia_image, render_with_skia},
    },
    raster::{BitmapFrame, RasterFrame, RectI},
    render::{RenderContext, surface::SurfacePool},
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Clone, Node)]
pub struct RasterMultiMerge {
    pub id: NodeId,

    #[property(expected = Float)]
    pub opacity: NodeProperty,
    #[property(expected = Int)]
    pub blend_mode: NodeProperty,

    #[input(kind = Raster, variadic)]
    pub layers: Vec<PortRef>,
}

impl Default for RasterMultiMerge {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            opacity: NodeProperty::Float(1.0),
            blend_mode: NodeProperty::Int(BlendMode::Normal as i64),
            layers: Vec::new(),
        }
    }
}

#[node_impl]
impl RasterMultiMerge {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<RasterFrame> {
        let opacity = self.resolve_opacity(ctx)? as f32;
        let blend_mode =
            BlendMode::try_from(self.resolve_blend_mode(ctx)? as usize).map_err(|err| {
                LumenError::Render(RenderError::NodeEvaluation {
                    frame: ctx.frame,
                    node_id: self.node_id(),
                    node_kind: "raster_multimerge",
                    details: err.into(),
                })
            })?;

        let mut rasters = Vec::new();
        for layer in &self.layers {
            if layer.is_empty() {
                continue;
            }
            let result = ctx.eval(layer.clone())?;
            rasters.push(result.as_raster()?.snapshot()?);
        }

        let mut rasters = rasters.into_iter();
        let Some(mut acc) = rasters.next() else {
            let w = ctx.renderer.composition.render_settings.width.max(1);
            let h = ctx.renderer.composition.render_settings.height.max(1);
            return Ok(RasterFrame::Bitmap(BitmapFrame::new(
                Arc::new(vec![0u8; (w as usize) * (h as usize) * 4]),
                w,
                h,
            )));
        };

        if opacity <= 0.0 {
            return Ok(acc);
        }

        let skia_blend: skia_safe::BlendMode = blend_mode.into();
        for overlay in rasters {
            acc = merge_pair(&acc, &overlay, opacity, skia_blend, ctx)?;
        }

        Ok(acc)
    }
}

fn merge_pair<S: SurfacePool, M: MediaStore>(
    base: &RasterFrame,
    overlay: &RasterFrame,
    opacity: f32,
    blend_mode: skia_safe::BlendMode,
    ctx: &mut RenderContext<'_, S, M>,
) -> crate::Result<RasterFrame> {
    let (base_bytes, base_w, base_h) = base.snapshot_parts()?;
    let (overlay_bytes, overlay_w, overlay_h) = overlay.snapshot_parts()?;
    let base_alpha = base.alpha_mode();
    let overlay_alpha = overlay.alpha_mode();
    let base_format = base.format_rect();
    let base_data = base.data_rect();
    let overlay_format = overlay.format_rect();
    let overlay_data = overlay.data_rect();

    let out_format = union_rect(base_format, overlay_format);
    let mut out_data = union_rect(base_data, overlay_data);
    out_data =
        out_format
            .intersect(&out_data)
            .unwrap_or(RectI::new(out_format.x, out_format.y, 0, 0));
    let render_w = out_data.width.max(1);
    let render_h = out_data.height.max(1);

    let base_image = make_skia_image(
        base_bytes.as_slice(),
        base_w,
        base_h,
        (base_w as usize) * 4,
        base_alpha,
    );
    let overlay_image = make_skia_image(
        overlay_bytes.as_slice(),
        overlay_w,
        overlay_h,
        (overlay_w as usize) * 4,
        overlay_alpha,
    );

    let Some(base_image) = base_image else {
        return Ok(RasterFrame::Bitmap(
            BitmapFrame::with_domain(
                Arc::new(vec![0u8; (render_w as usize) * (render_h as usize) * 4]),
                render_w,
                render_h,
                out_format,
                out_data,
            )
            .with_alpha_mode(base_alpha),
        ));
    };
    let Some(overlay_image) = overlay_image else {
        return base.snapshot();
    };

    let opacity = opacity.clamp(0.0, 1.0);
    let merged = render_with_skia(render_w, render_h, Some(ctx), |canvas| {
        draw_frame_image(
            canvas,
            &base_image,
            base_w,
            base_h,
            base_format,
            base_data,
            out_data,
            None,
        );

        let mut overlay_paint = Paint::default();
        overlay_paint.set_blend_mode(blend_mode);
        overlay_paint.set_alpha_f(opacity);
        draw_frame_image(
            canvas,
            &overlay_image,
            overlay_w,
            overlay_h,
            overlay_format,
            overlay_data,
            out_data,
            Some(&overlay_paint),
        );
    });

    Ok(RasterFrame::Bitmap(
        BitmapFrame::with_domain(Arc::new(merged), render_w, render_h, out_format, out_data)
            .with_alpha_mode(base_alpha),
    ))
}
