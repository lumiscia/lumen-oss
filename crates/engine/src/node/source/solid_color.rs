use crate::node::{Deferred, NodeId, NodeParamEvalContext, NodeParams};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuFrameBinding, RasterHandle,
    RasterMetadata, compiler,
};

pub(crate) const SHADER: &str = include_str!("solid_color.wgsl");

#[derive(Debug, Clone, lumen_macros::NodeParams)]
#[params(evaluated = EvaluatedSolidColorParams)]
#[cfg_attr(feature = "json", derive(serde::Deserialize), serde(default))]
pub struct SolidColorParams {
    /// Fill color.
    #[param(kind = "color")]
    pub color: Deferred<[u8; 4]>,
    /// Output width in pixels. Use 0 to match the composition width.
    #[param(kind = "int", min = 0, step = 1)]
    pub width: Deferred<i64>,
    /// Output height in pixels. Use 0 to match the composition height.
    #[param(kind = "int", min = 0, step = 1)]
    pub height: Deferred<i64>,
}

impl Default for SolidColorParams {
    fn default() -> Self {
        Self {
            color: Deferred::value([0, 0, 0, 255]),
            width: Deferred::value(0),
            height: Deferred::value(0),
        }
    }
}

/// Generates a solid raster texture.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "solid_color", name = "Solid Color", category = "source")]
pub struct SolidColor {
    pub id: NodeId,
    #[params]
    pub params: SolidColorParams,
}

impl Default for SolidColor {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: SolidColorParams::default(),
        }
    }
}

#[derive(Debug, Clone)]
struct SolidColorFrameBinding {
    node_id: NodeId,
    params: SolidColorParams,
    buffer: lumen_gpu::BufferId,
}

impl GpuFrameBinding for SolidColorFrameBinding {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let params = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(self.node_id, "params"),
        })?;
        bound.write_buffer(
            self.buffer,
            0,
            bytemuck::bytes_of(&compiler::ColorParams::from_rgba8(params.color)),
        );
        Ok(())
    }
}

impl GpuCompileNode for SolidColor {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &crate::node::PortRef,
    ) -> crate::Result<CompiledOutput> {
        if port.port != "output" {
            return Err(ctx.missing_output(self.id, &port.port));
        }

        let params = self.params.eval(&NodeParamEvalContext {
            node_id: self.id,
            expr: &ctx.expr_context(self.id, "params"),
        })?;
        let width = ctx.static_dimension_value(params.width, "width");
        let height = ctx.static_dimension_value(params.height, "height");
        let size = lumen_gpu::Size::new(width, height);
        let texture = ctx.builder_mut().texture_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("solid-color:{}:output", self.id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let buffer = ctx.builder_mut().buffer_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("solid-color:{}:params", self.id.0)),
            lumen_gpu::BufferDesc::uniform(std::mem::size_of::<compiler::ColorParams>() as u64),
        );
        let program = ctx.builder_mut().program_for(
            lumen_gpu::NodeKey(self.id.0),
            lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
                label: Some("solid-color".to_string()),
                shader: SHADER.to_string(),
                entry: "cs_main".to_string(),
                bind_groups: lumen_gpu::BindGroupLayoutSpec::single(vec![
                    lumen_gpu::BindingLayoutEntry::uniform(
                        0,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                    ),
                    lumen_gpu::BindingLayoutEntry::storage_texture(
                        1,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                        lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                        lumen_gpu::wgpu::StorageTextureAccess::WriteOnly,
                    ),
                ]),
            }),
        );
        ctx.builder_mut().compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("solid-color:{}:fill", self.id.0)),
            owner: Some(lumen_gpu::NodeKey(self.id.0)),
            program,
            bindings: vec![
                lumen_gpu::Binding::uniform(0, 0, buffer),
                lumen_gpu::Binding::storage_texture(0, 1, texture),
            ],
            dispatch: compiler::dispatch_for(size).into(),
        });
        ctx.builder_mut().param(
            lumen_gpu::ParamKey {
                owner: lumen_gpu::NodeKey(self.id.0),
                slot: 0,
            },
            lumen_gpu::ParamTarget::Buffer(buffer),
        );
        ctx.push_frame_binding(SolidColorFrameBinding {
            node_id: self.id,
            params: self.params.clone(),
            buffer,
        });

        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            metadata: RasterMetadata::default(),
        }))
    }
}
