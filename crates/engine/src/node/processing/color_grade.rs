use crate::node::{
    DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams, processing::RasterSampling,
};

use crate::gpu::params;

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("color_grade.wgsl")
);

pub const IDENTITY_LUT: &str = "identity";

/// Applies a LUT-driven color transform to a raster.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct ColorGradeParams {
    /// LUT data source or named LUT preset.
    #[meta(
        name = "LUT source",
        role = "lut_source",
        multiline,
        recommended_rows = 4
    )]
    pub lut_source: String,
    /// Blend amount for the LUT transform.
    #[meta(min = 0, max = 1, step = 0.01)]
    pub strength: f64,
    /// Sampling filter used when reading LUT data.
    #[meta(kind = "enum", enum_type = RasterSampling)]
    pub interpolation: RasterSampling,
}

impl Default for ColorGradeParams {
    fn default() -> Self {
        Self {
            lut_source: IDENTITY_LUT.to_string(),
            strength: 1.0,
            interpolation: RasterSampling::Linear,
        }
    }
}

/// Applies a LUT-driven color transform to a raster.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "color_grade", name = "Color Grade", category = "processing")]
pub struct ColorGrade {
    pub id: NodeId,
    #[params]
    pub params: ColorGradeParamsDelegate,

    #[input()]
    pub source: Option<NodeId>,
}

const RECORD_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(1, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::compute(2, lumen_gpu::LayoutKind::Storage),
    lumen_gpu::LayoutEntry::compute(
        3,
        lumen_gpu::LayoutKind::StorageTexture(lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
    ),
];

impl crate::gpu::frame_graph::RecordNode for ColorGrade {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.compute("color-grade", SHADER, "cs_main", RECORD_LAYOUT)
    }

    fn record(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        let source = cx.required_full(self.id, "source", self.source)?;
        self.grade(cx, source)
    }
}

impl ColorGrade {
    /// Grades `source`. It has a frame of its own, which is gone before the next node's input
    /// records, so the colour table doesn't add to the stack a long chain of nodes uses.
    #[inline(never)]
    fn grade(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
        source: crate::gpu::frame_graph::Raster,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        let evaluated = self.params.eval(&DelegateEvalContext {
            node_id: self.id,
            expr: &cx.expr(),
        })?;
        let gpu_params = params::ColorGradeParams {
            strength: evaluated.strength as f32,
            interpolation: evaluated.interpolation as u32,
        };
        let lut = params::ColorGradeLut::parse(self.id, &evaluated.lut_source)?;

        let size = source.domain.storage_size;
        let tex = cx.storage_target(size);
        let program = cx.program("color-grade", SHADER, "cs_main", RECORD_LAYOUT)?;
        let uniform = cx.rec.uniform(&gpu_params);
        let storage = cx.rec.storage_parts(&[bytemuck::bytes_of(&lut)]);
        cx.dispatch(
            program,
            &[
                (0, lumen_gpu::Res::Tex(source.tex)),
                (1, uniform),
                (2, storage),
                (3, lumen_gpu::Res::Tex(tex)),
            ],
            params::dispatch_for(size),
        );
        Ok(crate::gpu::frame_graph::Raster { tex, ..source })
    }
}
