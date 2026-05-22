use crate::node::{NodeId, NodeParamEvalContext, NodeParams, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuFrameBinding, RasterHandle,
    compiler,
};

pub(crate) const SHADER: &str = include_str!("crop.wgsl");

/// Extracts a fixed raster region into static output bounds.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct CropParams {
    /// Left edge of the crop region in pixels.
    #[meta(step = 1)]
    pub x: i64,
    /// Top edge of the crop region in pixels.
    #[meta(step = 1)]
    pub y: i64,
    /// Width of the crop region in pixels.
    #[meta(min = 0, step = 1)]
    pub width: i64,
    /// Height of the crop region in pixels.
    #[meta(min = 0, step = 1)]
    pub height: i64,
}

impl Default for CropParams {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        }
    }
}

/// Extracts a fixed raster region into static output bounds.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "crop", name = "Crop", category = "processing")]
pub struct Crop {
    pub id: NodeId,
    #[params]
    pub params: CropParamsDelegate,

    #[input()]
    pub source: PortRef,
}

impl Default for Crop {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: CropParamsDelegate::default(),
            source: PortRef::empty(),
        }
    }
}

impl GpuCompileNode for Crop {
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
        let size = lumen_gpu::Size::new(
            ctx.static_dimension(&self.params.width, self.id, "width")?,
            ctx.static_dimension(&self.params.height, self.id, "height")?,
        );
        let texture = ctx.builder_mut().texture_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("crop:{}:output", self.id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let params = ctx.builder_mut().buffer_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("crop:{}:params", self.id.0)),
            lumen_gpu::BufferDesc::uniform(std::mem::size_of::<compiler::CropParams>() as u64),
        );
        let program = ctx.spatial_program(self.id, "crop", SHADER);
        ctx.builder_mut().compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("crop:{}:apply", self.id.0)),
            owner: Some(lumen_gpu::NodeKey(self.id.0)),
            program,
            bindings: compiler::spatial_bindings(source.texture, params, texture),
            dispatch: compiler::dispatch_for(size).into(),
        });
        ctx.builder_mut().param(
            lumen_gpu::ParamKey {
                owner: lumen_gpu::NodeKey(self.id.0),
                slot: 0,
            },
            lumen_gpu::ParamTarget::Buffer(params),
        );
        ctx.push_frame_binding(CropFrameBinding {
            node_id: self.id,
            params: self.params.clone(),
            buffer: params,
        });

        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            metadata: source.metadata,
        }))
    }
}

#[derive(Debug, Clone)]
struct CropFrameBinding {
    node_id: NodeId,
    params: CropParamsDelegate,
    buffer: lumen_gpu::BufferId,
}

impl GpuFrameBinding for CropFrameBinding {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let params = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(self.node_id, "params"),
        })?;
        let gpu_params = compiler::CropParams {
            origin: [params.x as i32, params.y as i32],
            size: [params.width.max(0) as u32, params.height.max(0) as u32],
        };
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&gpu_params));
        Ok(())
    }
}
