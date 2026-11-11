use bytemuck::{Pod, Zeroable};

use crate::error::PropertyError;
use crate::gpu::params;
use crate::node::{
    DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams, vector::paint::Paint,
};

use super::displacement_map::EdgeMode;
use super::gaussian;

const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/color.wgsl"),
    include_str!("glow.wgsl")
);
/// Keeps the brightest light well inside the half-float range the bright pass stores it in.
const MAX_INTENSITY: f64 = 1000.0;

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct GlowParams {
    /// Luminance from 0 to 1 where the glow starts, on the sRGB-encoded scale, so 0.5 is half way
    /// up from black to white as they look. Brighter pixels glow more, white at full strength,
    /// and 1 lets nothing glow. Past 254/255 white dims too, so the glow fades out
    /// continuously as the threshold reaches 1.
    #[meta(min = 0, max = 1, step = 0.01)]
    pub threshold: f64,
    /// Glow blur radius in pixels, as on `blur`. Radii above about 588 look the same.
    #[meta(min = 0, step = 0.5)]
    pub radius: f64,
    /// Multiplies the glow's brightness, up to 1000.
    #[meta(min = 0, max = 1000, step = 0.05)]
    pub intensity: f64,
    /// Colours the glow with `tint` instead of the colours that cast it.
    #[meta()]
    pub tint_enabled: bool,
    /// Glow colour when `tint_enabled` is set, at the brightness of the light that casts it. Must
    /// be a solid color; a gradient is an error.
    #[meta(role = "color")]
    pub tint: Paint,
    /// Outputs only the glow instead of adding it to the source.
    #[meta()]
    pub glow_only: bool,
    /// What the source reads as beyond the canvas.
    #[meta()]
    pub edge: EdgeMode,
}

impl Default for GlowParams {
    fn default() -> Self {
        Self {
            threshold: 0.6,
            radius: 12.0,
            intensity: 1.0,
            tint_enabled: false,
            tint: Paint::solid([255, 255, 255, 255]),
            glow_only: false,
            edge: EdgeMode::Clamp,
        }
    }
}

/// Adds a blurred copy of a raster's brightest pixels to it, as light: the glow's alpha is its
/// brightest channel, so dark pixels add nothing.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "glow", name = "Glow", category = "processing")]
pub struct Glow {
    pub id: NodeId,
    #[params]
    pub params: GlowParamsDelegate,
    #[input()]
    pub source: Option<NodeId>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuGlowParams {
    /// Premultiplied tint, or zero to keep the source's colours.
    tint: [f32; 4],
    /// Threshold, intensity, and whether the glow is tinted.
    values: [f32; 4],
    /// Canvas pixels of the first texels of the output in `xy` and the source in `zw`.
    origins: [i32; 4],
    /// Canvas pixel of the glow's first texel in `xy`.
    glow_origin: [i32; 4],
}

const BRIGHT_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(2, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::compute(
        3,
        lumen_gpu::LayoutKind::StorageTexture(lumen_gpu::wgpu::TextureFormat::Rgba16Float),
    ),
];
const COMPOSITE_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(1, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(2, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::compute(4, crate::gpu::frame_graph::RECORD_STORAGE_TEXTURE),
];

fn finite_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() { value } else { fallback }
}

impl crate::gpu::frame_graph::RecordNode for Glow {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        gaussian::programs(programs, false)?;
        programs.compute("glow", SHADER, "bright_main", BRIGHT_LAYOUT)?;
        programs.compute("glow", SHADER, "composite_main", COMPOSITE_LAYOUT)
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
        let tint = if evaluated.tint_enabled {
            let Paint::SolidColor(r, g, b, a) = evaluated.tint else {
                return Err(PropertyError::InvalidType {
                    node_id: self.id,
                    property_path: "tint".to_string(),
                    expected: "a solid color",
                    actual: "a gradient",
                }
                .into());
            };
            let [r, g, b, _] = crate::gpu::color::linear_premultiplied([r, g, b, a]);
            [r, g, b]
        } else {
            [0.0; 3]
        };
        let display = source.domain.display_size();
        let glow_domain = cx.domain_within(
            display,
            cx.bounds_of(gaussian::blur_bounds(
                source.domain.data_rect,
                [0.0; 2],
                evaluated.radius,
            )),
        );
        let domain = if evaluated.glow_only {
            glow_domain
        } else {
            cx.domain_within(
                display,
                Some(source.domain.data_rect.union(glow_domain.data_rect)),
            )
        };
        let gpu_params = GpuGlowParams {
            tint: [tint[0], tint[1], tint[2], 0.0],
            values: [
                finite_or(evaluated.threshold, 1.0).clamp(0.0, 1.0) as f32,
                finite_or(evaluated.intensity, 0.0).clamp(0.0, MAX_INTENSITY) as f32,
                f32::from(u8::from(evaluated.tint_enabled)),
                0.0,
            ],
            origins: [
                domain.data_rect.x,
                domain.data_rect.y,
                source.domain.data_rect.x,
                source.domain.data_rect.y,
            ],
            glow_origin: [glow_domain.data_rect.x, glow_domain.data_rect.y, 0, 0],
        };
        let uniform = cx.rec.uniform(&gpu_params);

        // The bright pass keeps the source's domain, in float so that intensity can lift it
        // past 1 before the blur spreads it out.
        let bright_size = source.domain.storage_size;
        let bright = cx.float_target(bright_size);
        let program = cx.program("glow", SHADER, "bright_main", BRIGHT_LAYOUT)?;
        cx.dispatch(
            program,
            &[
                (0, lumen_gpu::Res::Tex(source.tex)),
                (2, uniform),
                (3, lumen_gpu::Res::Tex(bright)),
            ],
            params::dispatch_for(bright_size),
        );
        let glow = gaussian::record_passes(
            cx,
            crate::gpu::frame_graph::Raster {
                tex: bright,
                ..source
            },
            glow_domain,
            false,
            &gaussian::GaussianSettings {
                radius: evaluated.radius,
                transparent_edge: evaluated.edge == EdgeMode::Transparent,
                offset: [0.0; 2],
            },
        )?;
        if evaluated.glow_only {
            return Ok(crate::gpu::frame_graph::Raster {
                tex: glow,
                domain,
                bounds: None,
            });
        }

        let size = domain.storage_size;
        let tex = cx.storage_target(size);
        let program = cx.program("glow", SHADER, "composite_main", COMPOSITE_LAYOUT)?;
        cx.dispatch(
            program,
            &[
                (0, lumen_gpu::Res::Tex(source.tex)),
                (1, lumen_gpu::Res::Tex(glow)),
                (2, uniform),
                (4, lumen_gpu::Res::Tex(tex)),
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
