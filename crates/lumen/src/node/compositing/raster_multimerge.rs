use skia_safe::Paint;

use crate::{
    error::{LumenError, RenderError},
    node::{
        NodeId, NodeProperty, PortRef,
        compositing::{
            BlendMode,
            merge::{draw_frame_image, union_rect},
        },
        pixel_utils::{ClearMode, render_to_surface_ephemeral},
    },
    raster::{AlphaMode, RasterFrame, RectI},
    render::RenderContext,
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

        let mut layer_results = Vec::new();
        for layer in &self.layers {
            if layer.is_empty() {
                continue;
            }
            let result = ctx.eval(layer)?;
            layer_results.push(result);
        }

        let mut layer_refs: Vec<&RasterFrame> = Vec::new();
        for result in &layer_results {
            layer_refs.push(result.as_raster()?);
        }

        let mut iter = layer_refs.into_iter();
        let Some(first) = iter.next() else {
            let w = ctx.renderer.composition.render_settings.width.max(1);
            let h = ctx.renderer.composition.render_settings.height.max(1);
            return render_to_surface_ephemeral(
                w,
                h,
                ctx,
                RectI::from_size(w, h),
                RectI::from_size(w, h),
                AlphaMode::Premultiplied,
                ClearMode::Transparent,
                |_| {},
            );
        };

        let remaining: Vec<&RasterFrame> = iter.collect();
        if opacity <= 0.0 || remaining.is_empty() {
            return first.snapshot();
        }

        let base_alpha = first.alpha_mode();
        let mut out_format = first.format_rect();
        let mut out_data = first.data_rect();
        for layer in &remaining {
            out_format = union_rect(out_format, layer.format_rect());
            out_data = union_rect(out_data, layer.data_rect());
        }
        out_data =
            out_format
                .intersect(&out_data)
                .unwrap_or(RectI::new(out_format.x, out_format.y, 0, 0));
        let render_w = out_data.width.max(1);
        let render_h = out_data.height.max(1);
        let opacity = opacity.clamp(0.0, 1.0);
        let skia_blend: skia_safe::BlendMode = blend_mode.into();

        render_to_surface_ephemeral(
            render_w,
            render_h,
            ctx,
            out_format,
            out_data,
            base_alpha,
            ClearMode::Transparent,
            |canvas| {
                if let Some((image, width, height)) = first.image_parts() {
                    draw_frame_image(
                        canvas,
                        &image,
                        width,
                        height,
                        first.format_rect(),
                        first.data_rect(),
                        out_data,
                        None,
                    );
                }

                let mut overlay_paint = Paint::default();
                overlay_paint.set_blend_mode(skia_blend);
                overlay_paint.set_alpha_f(opacity);

                for layer in &remaining {
                    let Some((image, width, height)) = layer.image_parts() else {
                        continue;
                    };
                    draw_frame_image(
                        canvas,
                        &image,
                        width,
                        height,
                        layer.format_rect(),
                        layer.data_rect(),
                        out_data,
                        Some(&overlay_paint),
                    );
                }
            },
        )
    }
}
