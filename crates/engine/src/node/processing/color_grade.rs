use crate::node::{NodeId, NodeParamEvalContext, NodeParams, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuFrameBinding, RasterHandle,
    compiler,
};

pub(crate) const SHADER: &str = include_str!("color_grade.wgsl");

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
    #[meta(format = "sampling_mode")]
    pub interpolation: i64,
}

impl Default for ColorGradeParams {
    fn default() -> Self {
        Self {
            lut_source: IDENTITY_LUT.to_string(),
            strength: 1.0,
            interpolation: 1,
        }
    }
}

/// Applies a LUT-driven color transform to a raster.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "color_grade", name = "Color Grade", category = "processing")]
pub struct ColorGrade {
    pub id: NodeId,
    #[params]
    pub params: ColorGradeParamsDelegate,

    #[input()]
    pub source: PortRef,
}

impl Default for ColorGrade {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: ColorGradeParamsDelegate::default(),
            source: PortRef::empty(),
        }
    }
}

impl GpuCompileNode for ColorGrade {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        if port.port != "output" {
            return Err(ctx.missing_output(self.id, &port.port));
        }

        let source = ctx
            .compile_port(&self.source)?
            .into_raster(self.source.id, &self.source.port)?;
        let size = source.domain.storage_size;
        let texture = ctx.builder_mut().texture_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("color-grade:{}:output", self.id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let params =
            ctx.builder_mut().buffer_for(
                lumen_gpu::NodeKey(self.id.0),
                Some(format!("color-grade:{}:params", self.id.0)),
                lumen_gpu::BufferDesc::uniform(
                    std::mem::size_of::<compiler::ColorGradeParams>() as u64
                ),
            );
        let lut = ctx.builder_mut().buffer_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("color-grade:{}:lut", self.id.0)),
            lumen_gpu::BufferDesc::storage(std::mem::size_of::<compiler::ColorGradeLut>() as u64),
        );
        let program = ctx.builder_mut().program_for(
            lumen_gpu::NodeKey(self.id.0),
            lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
                label: Some("color-grade".to_string()),
                shader: SHADER.to_string(),
                entry: "cs_main".to_string(),
                bind_groups: lumen_gpu::BindGroupLayoutSpec::single(vec![
                    lumen_gpu::BindingLayoutEntry::texture(
                        0,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                    ),
                    lumen_gpu::BindingLayoutEntry::uniform(
                        1,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                    ),
                    lumen_gpu::BindingLayoutEntry::storage(
                        2,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                        true,
                    ),
                    lumen_gpu::BindingLayoutEntry::storage_texture(
                        3,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                        lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                        lumen_gpu::wgpu::StorageTextureAccess::WriteOnly,
                    ),
                ]),
            }),
        );
        ctx.builder_mut().compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("color-grade:{}:apply", self.id.0)),
            owner: Some(lumen_gpu::NodeKey(self.id.0)),
            program,
            bindings: vec![
                lumen_gpu::Binding::sampled_texture(0, 0, source.texture),
                lumen_gpu::Binding::uniform(0, 1, params),
                lumen_gpu::Binding::storage_buffer(0, 2, lut),
                lumen_gpu::Binding::storage_texture(0, 3, texture),
            ],
            dispatch: compiler::dispatch_for(size).into(),
        });
        ctx.builder_mut().param(
            lumen_gpu::ParamKey {
                owner: lumen_gpu::NodeKey(self.id.0),
                slot: 0,
            },
            lumen_gpu::ParamTarget::Buffer(params),
        );
        ctx.builder_mut().param(
            lumen_gpu::ParamKey {
                owner: lumen_gpu::NodeKey(self.id.0),
                slot: 1,
            },
            lumen_gpu::ParamTarget::Buffer(lut),
        );
        ctx.push_frame_binding(ColorGradeFrameBinding {
            node_id: self.id,
            params: self.params.clone(),
            params_buffer: params,
            lut_buffer: lut,
        });

        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: source.domain,
            metadata: source.metadata,
        }))
    }
}

#[derive(Debug, Clone)]
struct ColorGradeFrameBinding {
    node_id: NodeId,
    params: ColorGradeParamsDelegate,
    params_buffer: lumen_gpu::BufferId,
    lut_buffer: lumen_gpu::BufferId,
}

impl GpuFrameBinding for ColorGradeFrameBinding {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let evaluated = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(self.node_id, "params"),
        })?;
        let gpu_params = compiler::ColorGradeParams {
            strength: evaluated.strength as f32,
            interpolation: if evaluated.interpolation == 0 { 0 } else { 1 },
            _pad: [0; 2],
        };
        let lut = compiler::ColorGradeLut::parse(self.node_id, ctx.frame(), &evaluated.lut_source)?;
        bound.write_buffer(self.params_buffer, 0, bytemuck::bytes_of(&gpu_params));
        bound.write_buffer(self.lut_buffer, 0, bytemuck::bytes_of(&lut));
        Ok(())
    }
}
