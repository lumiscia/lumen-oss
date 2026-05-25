use crate::node::{NodeId, NodeParamEvalContext, NodeParams, PortRef, vector::paint::Paint};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode, RasterHandle,
    compiler,
};

pub(crate) const SHADER: &str = include_str!("shadow.wgsl");

/// Composites a blurred alpha shadow behind a raster.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct ShadowParams {
    /// Horizontal shadow offset in pixels.
    #[meta(name = "Offset X", step = 1)]
    pub offset_x: f64,
    /// Vertical shadow offset in pixels.
    #[meta(name = "Offset Y", step = 1)]
    pub offset_y: f64,
    /// Shadow blur radius in pixels.
    #[meta(name = "Blur radius", min = 0, step = 0.5)]
    pub radius: f64,
    /// Shadow color.
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
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "shadow", name = "Shadow", category = "processing")]
pub struct Shadow {
    pub id: NodeId,
    #[params]
    pub params: ShadowParamsDelegate,

    #[input()]
    pub source: PortRef,
}

impl Default for Shadow {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: ShadowParamsDelegate::default(),
            source: PortRef::empty(),
        }
    }
}

impl GpuCompileNode for Shadow {
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
        let temp = ctx.builder_mut().texture_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("shadow:{}:horizontal", self.id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let texture = ctx.builder_mut().texture_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("shadow:{}:output", self.id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let params = ctx.builder_mut().buffer_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("shadow:{}:params", self.id.0)),
            lumen_gpu::BufferDesc::uniform(std::mem::size_of::<compiler::ShadowParams>() as u64),
        );
        let bind_groups = lumen_gpu::BindGroupLayoutSpec::single(vec![
            lumen_gpu::BindingLayoutEntry::texture(0, lumen_gpu::wgpu::ShaderStages::COMPUTE),
            lumen_gpu::BindingLayoutEntry::texture(1, lumen_gpu::wgpu::ShaderStages::COMPUTE),
            lumen_gpu::BindingLayoutEntry::uniform(2, lumen_gpu::wgpu::ShaderStages::COMPUTE),
            lumen_gpu::BindingLayoutEntry::storage_texture(
                3,
                lumen_gpu::wgpu::ShaderStages::COMPUTE,
                lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                lumen_gpu::wgpu::StorageTextureAccess::WriteOnly,
            ),
        ]);
        let horizontal = ctx.builder_mut().program_for(
            lumen_gpu::NodeKey(self.id.0),
            lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
                label: Some("shadow-horizontal".to_string()),
                shader: SHADER.to_string(),
                entry: "horizontal_main".to_string(),
                bind_groups: bind_groups.clone(),
            }),
        );
        let vertical = ctx.builder_mut().program_for(
            lumen_gpu::NodeKey(self.id.0),
            lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
                label: Some("shadow-vertical".to_string()),
                shader: SHADER.to_string(),
                entry: "vertical_main".to_string(),
                bind_groups,
            }),
        );
        ctx.builder_mut().compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("shadow:{}:horizontal", self.id.0)),
            owner: Some(lumen_gpu::NodeKey(self.id.0)),
            program: horizontal,
            bindings: vec![
                lumen_gpu::Binding::sampled_texture(0, 0, source.texture),
                lumen_gpu::Binding::sampled_texture(0, 1, source.texture),
                lumen_gpu::Binding::uniform(0, 2, params),
                lumen_gpu::Binding::storage_texture(0, 3, temp),
            ],
            dispatch: compiler::dispatch_for(size).into(),
        });
        ctx.builder_mut().compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("shadow:{}:vertical", self.id.0)),
            owner: Some(lumen_gpu::NodeKey(self.id.0)),
            program: vertical,
            bindings: vec![
                lumen_gpu::Binding::sampled_texture(0, 0, source.texture),
                lumen_gpu::Binding::sampled_texture(0, 1, temp),
                lumen_gpu::Binding::uniform(0, 2, params),
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
        ctx.register_compiled_node(CompiledShadow {
            node_id: self.id,
            params: self.params.clone(),
            buffer: params,
        });
        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: source.domain,
            metadata: source.metadata,
        }))
    }
}

#[derive(Debug, Clone)]
struct CompiledShadow {
    node_id: NodeId,
    params: ShadowParamsDelegate,
    buffer: lumen_gpu::BufferId,
}

impl GpuCompiledNode for CompiledShadow {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let evaluated = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(self.node_id, "params"),
        })?;
        let color = evaluated.color.to_gpu([0, 0, 0, 255]).colors[0];
        let gpu_params = compiler::ShadowParams {
            color,
            values: [
                evaluated.offset_x as f32,
                evaluated.offset_y as f32,
                evaluated.radius.round().clamp(0.0, 32.0) as f32,
                evaluated.opacity as f32,
            ],
        };
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&gpu_params));
        Ok(())
    }
}
