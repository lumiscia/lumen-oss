use crate::node::{
    DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams, vector::paint::Paint,
};

use crate::gpu::{BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode};

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
        let params = self.params.eval(&DelegateEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(),
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

        let params = self.params.eval(&DelegateEvalContext {
            node_id: self.id,
            expr: &ctx.expr_context(),
        })?;
        let width = ctx.static_dimension_value(i64::from(params.width), "width");
        let height = ctx.static_dimension_value(i64::from(params.height), "height");
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

        Ok(CompiledOutput::Raster(source.output))
    }
}
