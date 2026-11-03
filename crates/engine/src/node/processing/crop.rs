use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams};

use crate::gpu::{
    BoundFrame, FrameBindContext, GpuCompileNode, GpuCompiledNode, RasterHandle, compiler,
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
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "crop", name = "Crop", category = "processing")]
pub struct Crop {
    pub id: NodeId,
    #[params]
    pub params: CropParamsDelegate,

    #[input()]
    pub source: Option<NodeId>,
}

impl GpuCompileNode for Crop {
    fn compile_gpu(&self, ctx: &mut crate::gpu::CompileContext<'_>) -> crate::Result<RasterHandle> {
        let source = ctx.compile_input(self.id, "source", self.source)?;
        let size = source.domain.storage_size;
        let crop = ctx.compute_source(
            self.id,
            "crop",
            "apply",
            SHADER,
            std::mem::size_of::<compiler::CropParams>() as u64,
            Some(source.texture),
            [],
            lumen_gpu::TextureDomain::full_frame(size),
        );
        ctx.register_compiled_node(CompiledCrop {
            node_id: self.id,
            params: self.params.clone(),
            buffer: crop.params,
            source_size: size,
        });

        Ok(RasterHandle {
            // The params buffer holds the region clamped to the source, which is the output's
            // logical bounds.
            bounds: Some(crop.params),
            ..crop.output
        })
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
        let params = self.params.eval(&DelegateEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(),
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
