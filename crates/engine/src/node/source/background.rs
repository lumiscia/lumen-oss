use crate::node::{NodeId, NodeParamEvalContext, NodeParams, vector::paint::Paint};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode, RasterHandle,
    RasterMetadata, compiler,
};

pub(crate) const SHADER: &str = include_str!("background.wgsl");

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct BackgroundParams {
    /// Background paint.
    #[meta()]
    pub paint: Paint,
    /// Output width in pixels. Use 0 to match the composition width.
    #[meta(min = 0, step = 1)]
    pub width: u32,
    /// Output height in pixels. Use 0 to match the composition height.
    #[meta(min = 0, step = 1)]
    pub height: u32,
    /// Enables 4×4 supersampling when evaluating paint at each pixel.
    #[meta()]
    pub paint_supersample: bool,
}

impl Default for BackgroundParams {
    fn default() -> Self {
        Self {
            paint: Paint::solid([0, 0, 0, 255]),
            width: 0,
            height: 0,
            paint_supersample: true,
        }
    }
}

/// Generates a background raster texture.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "background", name = "Background", category = "source")]
pub struct Background {
    pub id: NodeId,
    #[params]
    pub params: BackgroundParamsDelegate,
}

impl Default for Background {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: BackgroundParamsDelegate::default(),
        }
    }
}

#[derive(Debug, Clone)]
struct CompiledBackground {
    node_id: NodeId,
    params: BackgroundParamsDelegate,
    buffer: lumen_gpu::BufferId,
}

impl GpuCompiledNode for CompiledBackground {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let params = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(self.node_id, "params"),
        })?;
        let mut paint = params.paint.to_gpu([0, 0, 0, 255]);
        paint.paint_supersample = u32::from(params.paint_supersample);
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&paint));
        Ok(())
    }
}

impl GpuCompileNode for Background {
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
        let width = ctx.static_dimension_value(i64::from(params.width), "width");
        let height = ctx.static_dimension_value(i64::from(params.height), "height");
        let size = lumen_gpu::Size::new(width, height);
        let texture = ctx.builder_mut().texture_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("background:{}:output", self.id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let buffer = ctx.builder_mut().buffer_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("background:{}:params", self.id.0)),
            lumen_gpu::BufferDesc::uniform(
                std::mem::size_of::<crate::node::vector::paint::GpuPaint>() as u64,
            ),
        );
        let program = ctx.builder_mut().program_for(
            lumen_gpu::NodeKey(self.id.0),
            lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
                label: Some("background".to_string()),
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
            label: Some(format!("background:{}:fill", self.id.0)),
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
        ctx.register_compiled_node(CompiledBackground {
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
