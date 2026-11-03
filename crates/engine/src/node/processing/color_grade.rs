use crate::node::{
    DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams, processing::RasterSampling,
};

use crate::gpu::compiler::Slot;
use crate::gpu::{
    BoundFrame, FrameBindContext, GpuCompileNode, GpuCompiledNode, RasterHandle, compiler,
};

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

impl GpuCompileNode for ColorGrade {
    fn compile_gpu(&self, ctx: &mut crate::gpu::CompileContext<'_>) -> crate::Result<RasterHandle> {
        let source = ctx.compile_input(self.id, "source", self.source)?;
        let size = source.domain.storage_size;
        let texture = ctx.builder_mut().texture(
            Some(format!("color-grade:{}:output", self.id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let params =
            ctx.builder_mut().buffer(
                Some(format!("color-grade:{}:params", self.id.0)),
                lumen_gpu::BufferDesc::uniform(
                    std::mem::size_of::<compiler::ColorGradeParams>() as u64
                ),
            );
        let lut = ctx.builder_mut().buffer(
            Some(format!("color-grade:{}:lut", self.id.0)),
            lumen_gpu::BufferDesc::storage(std::mem::size_of::<compiler::ColorGradeLut>() as u64),
        );
        let program = ctx.compute_program(
            "color-grade",
            SHADER,
            &[
                Slot::Texture,
                Slot::Uniform,
                Slot::StorageRead,
                Slot::Output,
            ],
        );
        ctx.builder_mut().compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("color-grade:{}:apply", self.id.0)),
            program,
            bindings: vec![
                lumen_gpu::Binding::sampled_texture(0, 0, source.texture),
                lumen_gpu::Binding::uniform(0, 1, params),
                lumen_gpu::Binding::storage_buffer(0, 2, lut),
                lumen_gpu::Binding::storage_texture(0, 3, texture),
            ],
            dispatch: compiler::dispatch_for(size),
        });
        ctx.register_compiled_node(CompiledColorGrade {
            node_id: self.id,
            params: self.params.clone(),
            params_buffer: params,
            lut_buffer: lut,
        });

        Ok(RasterHandle {
            texture,
            domain: source.domain,
            bounds: source.bounds,
        })
    }
}

#[derive(Debug, Clone)]
struct CompiledColorGrade {
    node_id: NodeId,
    params: ColorGradeParamsDelegate,
    params_buffer: lumen_gpu::BufferId,
    lut_buffer: lumen_gpu::BufferId,
}

impl GpuCompiledNode for CompiledColorGrade {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let evaluated = self.params.eval(&DelegateEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(),
        })?;
        let gpu_params = compiler::ColorGradeParams {
            strength: evaluated.strength as f32,
            interpolation: evaluated.interpolation as u32,
        };
        let lut = compiler::ColorGradeLut::parse(self.node_id, &evaluated.lut_source)?;
        bound.write_buffer(self.params_buffer, 0, bytemuck::bytes_of(&gpu_params));
        bound.write_buffer(self.lut_buffer, 0, bytemuck::bytes_of(&lut));
        Ok(())
    }
}
