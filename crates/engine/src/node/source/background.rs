use crate::node::{
    DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams, vector::paint::Paint,
};

use crate::gpu::{BoundFrame, FrameBindContext, GpuCompileNode, GpuCompiledNode, RasterHandle};

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("../vector/paint/paint.wgsl"),
    include_str!("background.wgsl")
);

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct BackgroundParams {
    /// Background paint.
    #[meta()]
    pub paint: Paint,
    /// Output width in pixels. Use 0 to match the composition width.
    #[meta(min = 0, step = 1, constant)]
    pub width: u32,
    /// Output height in pixels. Use 0 to match the composition height.
    #[meta(min = 0, step = 1, constant)]
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
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "background", name = "Background", category = "source")]
pub struct Background {
    pub id: NodeId,
    #[params]
    pub params: BackgroundParamsDelegate,
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
        let params = self.params.eval(&DelegateEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(),
        })?;
        let mut paint = params.paint.to_gpu();
        paint.paint_supersample = u32::from(params.paint_supersample);
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&paint));
        Ok(())
    }
}

impl GpuCompileNode for Background {
    fn compile_gpu(&self, ctx: &mut crate::gpu::CompileContext<'_>) -> crate::Result<RasterHandle> {
        let params = self.params.eval(&DelegateEvalContext {
            node_id: self.id,
            expr: &ctx.expr_context(),
        })?;
        let canvas = ctx.canvas_size();
        let or_canvas = |size: u32, canvas: u32| if size == 0 { canvas } else { size };
        let width = or_canvas(params.width, canvas.width);
        let height = or_canvas(params.height, canvas.height);
        let source = ctx.compute_source(
            self.id,
            "background",
            "fill",
            SHADER,
            std::mem::size_of::<crate::node::vector::paint::GpuPaint>() as u64,
            None,
            [],
            lumen_gpu::TextureDomain::full_frame(lumen_gpu::Size::new(width, height)),
        );
        ctx.register_compiled_node(CompiledBackground {
            node_id: self.id,
            params: self.params.clone(),
            buffer: source.params,
        });

        Ok(source.output)
    }
}
