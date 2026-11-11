use crate::error::PropertyError;
use crate::node::{
    DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams, vector::paint::Paint,
};

use crate::gpu::params;

use super::gaussian;

const COMPOSITE_SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("../../gpu/shaders/color.wgsl"),
    include_str!("shadow.wgsl")
);

/// Composites a blurred alpha shadow behind a raster.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct ShadowParams {
    /// Horizontal shadow offset in pixels.
    #[meta(name = "Offset X", step = 1)]
    pub offset_x: f64,
    /// Vertical shadow offset in pixels.
    #[meta(name = "Offset Y", step = 1)]
    pub offset_y: f64,
    /// Shadow blur radius in pixels. Fractional radii blend smoothly. Radii above about 588 look the same.
    #[meta(name = "Blur radius", min = 0, step = 0.5)]
    pub radius: f64,
    /// Shadow color. Must be a solid color; a gradient is an error.
    #[meta(role = "color")]
    pub color: Paint,
    /// Shadow opacity.
    #[meta(min = 0, max = 1, step = 0.05)]
    pub opacity: f64,
}

impl Default for ShadowParams {
    fn default() -> Self {
        Self {
            offset_x: 8.0,
            offset_y: 8.0,
            radius: 8.0,
            color: Paint::solid([0, 0, 0, 255]),
            opacity: 0.5,
        }
    }
}

/// Composites a blurred alpha shadow behind a raster.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "shadow", name = "Shadow", category = "processing")]
pub struct Shadow {
    pub id: NodeId,
    #[params]
    pub params: ShadowParamsDelegate,

    #[input()]
    pub source: Option<NodeId>,
}

const RECORD_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(1, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(2, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::compute(3, crate::gpu::frame_graph::RECORD_STORAGE_TEXTURE),
];

impl crate::gpu::frame_graph::RecordNode for Shadow {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        gaussian::programs(programs, true)?;
        programs.compute(
            "shadow-composite",
            COMPOSITE_SHADER,
            "composite_main",
            RECORD_LAYOUT,
        )
    }

    fn record(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        let source = cx.required(self.id, "source", self.source)?;
        let evaluated = self.params.eval(&DelegateEvalContext {
            node_id: self.id,
            expr: &cx.expr(),
        })?;
        let bounds = cx.bounds_of(gaussian::blur_bounds(
            source.domain.data_rect,
            [evaluated.offset_x, evaluated.offset_y],
            evaluated.radius,
        ));
        let display = source.domain.display_size();
        let shadow_domain = cx.domain_within(display, bounds);
        // The source is composited over its shadow, so both are written.
        let domain = cx.domain_within(
            display,
            Some(source.domain.data_rect.union(shadow_domain.data_rect)),
        );
        let blurred = gaussian::record_passes(
            cx,
            source,
            shadow_domain,
            true,
            &gaussian::GaussianSettings {
                radius: evaluated.radius,
                transparent_edge: true,
                offset: [evaluated.offset_x, evaluated.offset_y],
            },
        )?;
        let Paint::SolidColor(r, g, b, a) = evaluated.color else {
            return Err(PropertyError::InvalidType {
                node_id: self.id,
                property_path: "color".to_string(),
                expected: "a solid color",
                actual: "a gradient",
            }
            .into());
        };
        let color =
            [r, g, b].map(|channel| crate::gpu::color::srgb_to_linear(f32::from(channel) / 255.0));
        let opacity = evaluated.opacity.clamp(0.0, 1.0) as f32 * f32::from(a) / 255.0;
        let params = params::ShadowParams {
            color: [color[0], color[1], color[2], opacity],
            origins: [
                domain.data_rect.x,
                domain.data_rect.y,
                shadow_domain.data_rect.x,
                shadow_domain.data_rect.y,
            ],
            source_origin: [source.domain.data_rect.x, source.domain.data_rect.y, 0, 0],
        };
        let size = domain.storage_size;
        let tex = cx.storage_target(size);
        let program = cx.program(
            "shadow-composite",
            COMPOSITE_SHADER,
            "composite_main",
            RECORD_LAYOUT,
        )?;
        let uniform = cx.rec.uniform(&params);
        cx.dispatch(
            program,
            &[
                (0, lumen_gpu::Res::Tex(blurred)),
                (1, lumen_gpu::Res::Tex(source.tex)),
                (2, uniform),
                (3, lumen_gpu::Res::Tex(tex)),
            ],
            params::dispatch_for(size),
        );
        Ok(crate::gpu::frame_graph::Raster {
            tex,
            domain,
            bounds: None,
        })
    }
}
