use crate::node::{Deferred, NodeId, NodeParams, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuFrameBinding, RasterHandle,
    compiler,
};

pub(crate) const SHADER: &str = include_str!("curves.wgsl");

/// Applies a 1D RGB curve table to a raster.
#[derive(Debug, Clone, lumen_macros::NodeParams)]
#[params(evaluated = EvaluatedCurvesParams)]
#[cfg_attr(feature = "json", derive(serde::Deserialize), serde(default))]
pub struct CurvesParams {
    /// Curve table data source or named curve preset.
    #[param(
        kind = "string",
        name = "Curve",
        role = "curve_source",
        multiline,
        recommended_rows = 4
    )]
    pub curve_source: Deferred<String>,
    /// Blend amount for the curve adjustment.
    #[param(kind = "float", min = 0, max = 1, step = 0.01)]
    pub strength: Deferred<f64>,
}

impl Default for CurvesParams {
    fn default() -> Self {
        Self {
            curve_source: Deferred::value("identity".to_string()),
            strength: Deferred::value(1.0),
        }
    }
}

/// Applies a 1D RGB curve table to a raster.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "curves", name = "Curves", category = "processing")]
pub struct Curves {
    pub id: NodeId,
    #[params]
    pub params: CurvesParams,

    #[input()]
    pub source: PortRef,
}

impl Default for Curves {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: CurvesParams::default(),
            source: PortRef::empty(),
        }
    }
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
        let texture = ctx.builder_mut().texture_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("curves:{}:output", self.id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let params = ctx.builder_mut().buffer_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("curves:{}:params", self.id.0)),
            lumen_gpu::BufferDesc::uniform(std::mem::size_of::<compiler::CurvesParams>() as u64),
        );
        let curve = ctx.builder_mut().buffer_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("curves:{}:table", self.id.0)),
            lumen_gpu::BufferDesc::storage(std::mem::size_of::<compiler::CurvesTable>() as u64),
        );
        let program = ctx.builder_mut().program_for(
            lumen_gpu::NodeKey(self.id.0),
            lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
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
            }),
        );
        ctx.builder_mut().compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("curves:{}:apply", self.id.0)),
            owner: Some(lumen_gpu::NodeKey(self.id.0)),
            program,
            bindings: vec![
                lumen_gpu::Binding::sampled_texture(0, 0, source.texture),
                lumen_gpu::Binding::uniform(0, 1, params),
                lumen_gpu::Binding::storage_buffer(0, 2, curve),
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
            lumen_gpu::ParamTarget::Buffer(curve),
        );
        ctx.push_frame_binding(CurvesFrameBinding {
            node_id: self.id,
            curve_source: self.params.curve_source.clone(),
            strength: self.params.strength.clone(),
            params_buffer: params,
            curve_buffer: curve,
        });

        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: source.domain,
            metadata: source.metadata,
        }))
    }
}

#[derive(Debug, Clone)]
struct CurvesFrameBinding {
    node_id: NodeId,
    curve_source: Deferred<String>,
    strength: Deferred<f64>,
    params_buffer: lumen_gpu::BufferId,
    curve_buffer: lumen_gpu::BufferId,
}

impl GpuFrameBinding for CurvesFrameBinding {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let curve_source = self.curve_source.resolve_string(
            self.node_id,
            "curve_source",
            &ctx.expr_context(self.node_id, "curve_source"),
        )?;
        let params = compiler::CurvesParams {
            values: [
                self.strength.resolve_float(
                    self.node_id,
                    "strength",
                    &ctx.expr_context(self.node_id, "strength"),
                )? as f32,
                0.0,
                0.0,
                0.0,
            ],
        };
        let curve = compiler::CurvesTable::parse(self.node_id, ctx.frame(), &curve_source)?;
        bound.write_buffer(self.params_buffer, 0, bytemuck::bytes_of(&params));
        bound.write_buffer(self.curve_buffer, 0, bytemuck::bytes_of(&curve));
        Ok(())
    }
}
