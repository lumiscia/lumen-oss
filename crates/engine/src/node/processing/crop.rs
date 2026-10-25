use crate::node::{NodeId, NodeParamEvalContext, NodeParams, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode, RasterHandle,
    compiler,
};

pub(crate) const SHADER: &str = include_str!("crop.wgsl");

/// Keeps a raster region in place and clears everything outside it.
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

/// Keeps a raster region in place and clears everything outside it.
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
        let size = source.domain.storage_size;
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
        // The params buffer holds the region clamped to the source, which is the output's logical
        // bounds. Only a `resize` wired directly to this node sees them.
        ctx.publish_raster_bounds(texture, params);
        ctx.register_compiled_node(CompiledCrop {
            node_id: self.id,
            params: self.params.clone(),
            buffer: params,
            source_size: size,
        });

        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            metadata: source.metadata,
        }))
    }
}

#[derive(Debug, Clone)]
struct CompiledCrop {
    node_id: NodeId,
    params: CropParamsDelegate,
    buffer: lumen_gpu::BufferId,
    source_size: lumen_gpu::Size,
}

impl GpuCompiledNode for CompiledCrop {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let params = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(self.node_id, "params"),
        })?;
        let x = compiler::pixel_origin(self.node_id, "x", params.x)?;
        let y = compiler::pixel_origin(self.node_id, "y", params.y)?;
        let width = compiler::pixel_extent(self.node_id, "width", params.width, 0)?;
        let height = compiler::pixel_extent(self.node_id, "height", params.height, 0)?;
        // The region is clamped to the source so it doubles as the output's logical bounds.
        let left = i64::from(x).clamp(0, i64::from(self.source_size.width));
        let top = i64::from(y).clamp(0, i64::from(self.source_size.height));
        let right =
            (i64::from(x) + i64::from(width)).clamp(left, i64::from(self.source_size.width));
        let bottom =
            (i64::from(y) + i64::from(height)).clamp(top, i64::from(self.source_size.height));
        let gpu_params = compiler::CropParams {
            origin: [left as i32, top as i32],
            size: [(right - left) as u32, (bottom - top) as u32],
        };
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&gpu_params));
        Ok(())
    }
}
