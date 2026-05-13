use crate::node::{NodeId, NodeProperty, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, FrameBinding, GpuCompileNode, GpuFrameBindNode,
    RasterHandle, compiler,
};

pub(crate) const SHADER: &str = include_str!("color_grade.wgsl");

pub const IDENTITY_LUT: &str = "identity";

/// Applies a LUT-driven color transform to a raster.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "color_grade", name = "Color Grade", category = "processing")]
pub struct ColorGrade {
    pub id: NodeId,
    /// LUT data source or named LUT preset.
    #[property(
        kind = "string",
        name = "LUT source",
        role = "lut_source",
        multiline,
        recommended_rows = 4
    )]
    pub lut_source: NodeProperty,
    /// Blend amount for the LUT transform.
    #[property(kind = "float", min = 0, max = 1, step = 0.01)]
    pub strength: NodeProperty,
    /// Sampling filter used when reading LUT data.
    #[property(kind = "int", format = "sampling_mode")]
    pub interpolation: NodeProperty,
    #[input()]
    pub source: PortRef,
}

impl Default for ColorGrade {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            lut_source: NodeProperty::String(IDENTITY_LUT.to_string()),
            strength: NodeProperty::Float(1.0),
            interpolation: NodeProperty::Int(1),
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
        ctx.push_frame_binding(FrameBinding::ColorGrade {
            node_id: self.id,
            lut_source: self.lut_source.clone(),
            strength: self.strength.clone(),
            interpolation: self.interpolation.clone(),
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

impl GpuFrameBindNode for ColorGrade {
    fn bind_gpu_frame(
        &self,
        ctx: &FrameBindContext<'_>,
        binding: &FrameBinding,
        bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        let FrameBinding::ColorGrade {
            node_id,
            lut_source,
            strength,
            interpolation,
            params_buffer,
            lut_buffer,
        } = binding
        else {
            return Ok(());
        };
        let lut_source = lut_source.resolve_string(
            *node_id,
            "lut_source",
            &ctx.expr_context(*node_id, "lut_source"),
        )?;
        let interpolation = interpolation.resolve_int(
            *node_id,
            "interpolation",
            &ctx.expr_context(*node_id, "interpolation"),
        )?;
        let params = compiler::ColorGradeParams {
            strength: strength.resolve_float(
                *node_id,
                "strength",
                &ctx.expr_context(*node_id, "strength"),
            )? as f32,
            interpolation: if interpolation == 0 { 0 } else { 1 },
            _pad: [0; 2],
        };
        let lut = compiler::ColorGradeLut::parse(*node_id, ctx.frame(), &lut_source)?;
        bound.write_buffer(*params_buffer, 0, bytemuck::bytes_of(&params));
        bound.write_buffer(*lut_buffer, 0, bytemuck::bytes_of(&lut));
        Ok(())
    }
}
