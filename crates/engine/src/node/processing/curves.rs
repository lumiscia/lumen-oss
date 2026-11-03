use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams, PortRef};

use crate::gpu::{
    AlphaMode, BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode,
    RasterHandle, compiler,
};

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("curves.wgsl")
);

/// Applies a 1D RGB curve table to a raster.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct CurvesParams {
    /// Curve table data source or named curve preset.
    #[meta(name = "Curve", role = "curve_source", multiline, recommended_rows = 4)]
    pub curve_source: String,
    /// Blend amount for the curve adjustment.
    #[meta(min = 0, max = 1, step = 0.01)]
    pub strength: f64,
}

impl Default for CurvesParams {
    fn default() -> Self {
        Self {
            curve_source: "identity".to_string(),
            strength: 1.0,
        }
    }
}

/// Applies a 1D RGB curve table to a raster.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "curves", name = "Curves", category = "processing")]
pub struct Curves {
    pub id: NodeId,
    #[params]
    pub params: CurvesParamsDelegate,

    #[input()]
    pub source: PortRef,
}

impl GpuCompileNode for Curves {
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
        let texture = ctx.builder_mut().texture(
            Some(format!("curves:{}:output", self.id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let params = ctx.builder_mut().buffer(
            Some(format!("curves:{}:params", self.id.0)),
            lumen_gpu::BufferDesc::uniform(std::mem::size_of::<compiler::CurvesParams>() as u64),
        );
        let curve = ctx.builder_mut().buffer(
            Some(format!("curves:{}:table", self.id.0)),
            lumen_gpu::BufferDesc::storage(std::mem::size_of::<compiler::CurvesTable>() as u64),
        );
        let program = ctx.builder_mut().program(lumen_gpu::ProgramDesc::Compute(
            lumen_gpu::ComputeProgramDesc {
                label: Some("curves".to_string()),
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
            },
        ));
        ctx.builder_mut().compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("curves:{}:apply", self.id.0)),
            program,
            bindings: vec![
                lumen_gpu::Binding::sampled_texture(0, 0, source.texture),
                lumen_gpu::Binding::uniform(0, 1, params),
                lumen_gpu::Binding::storage_buffer(0, 2, curve),
                lumen_gpu::Binding::storage_texture(0, 3, texture),
            ],
            dispatch: compiler::dispatch_for(size),
        });
        ctx.register_compiled_node(CompiledCurves {
            node_id: self.id,
            params: self.params.clone(),
            alpha_mode: source.metadata.alpha_mode,
            params_buffer: params,
            curve_buffer: curve,
        });

        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: source.domain,
            metadata: source.metadata,
            bounds: source.bounds,
        }))
    }
}

#[derive(Debug, Clone)]
struct CompiledCurves {
    node_id: NodeId,
    params: CurvesParamsDelegate,
    alpha_mode: AlphaMode,
    params_buffer: lumen_gpu::BufferId,
    curve_buffer: lumen_gpu::BufferId,
}

impl GpuCompiledNode for CompiledCurves {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let evaluated = self.params.eval(&DelegateEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(),
        })?;
        let gpu_params = compiler::CurvesParams {
            values: [
                evaluated.strength as f32,
                if self.alpha_mode == AlphaMode::Premultiplied {
                    1.0
                } else {
                    0.0
                },
                0.0,
                0.0,
            ],
        };
        let curve = compiler::CurvesTable::parse(self.node_id, &evaluated.curve_source)?;
        bound.write_buffer(self.params_buffer, 0, bytemuck::bytes_of(&gpu_params));
        bound.write_buffer(self.curve_buffer, 0, bytemuck::bytes_of(&curve));
        Ok(())
    }
}
