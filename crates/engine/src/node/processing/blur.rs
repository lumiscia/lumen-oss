use crate::node::{NodeId, NodeParamEvalContext, NodeParams, PortRef};

use crate::gpu::{
    AlphaMode, BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode,
    RasterHandle, RasterMetadata, compiler,
};

use super::gaussian;

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("gaussian.wgsl"),
    include_str!("blur.wgsl")
);

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct BlurParams {
    /// Blur radius in pixels. Fractional radii blend smoothly.
    #[meta(min = 0, step = 0.5)]
    pub radius: f64,
}

impl Default for BlurParams {
    fn default() -> Self {
        Self { radius: 4.0 }
    }
}

/// Applies a separable Gaussian blur to a raster.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "blur", name = "Blur", category = "processing")]
pub struct Blur {
    pub id: NodeId,
    #[params]
    pub params: BlurParamsDelegate,
    #[input()]
    pub source: PortRef,
}

impl Default for Blur {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: BlurParamsDelegate::default(),
            source: PortRef::empty(),
        }
    }
}

#[derive(Debug, Clone)]
struct CompiledBlur {
    node_id: NodeId,
    params: BlurParamsDelegate,
    buffer: lumen_gpu::BufferId,
    straight_source: bool,
}

impl GpuCompiledNode for CompiledBlur {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let params = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(self.node_id, "params"),
        })?;
        let params = compiler::BlurParams {
            values: [
                gaussian::sigma_for_radius(params.radius),
                if self.straight_source { 1.0 } else { 0.0 },
                0.0,
                0.0,
            ],
        };
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}

impl GpuCompileNode for Blur {
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
        let passes = gaussian::compile_passes(
            ctx,
            self.id,
            "blur",
            SHADER,
            source.texture,
            source.domain.storage_size,
            std::mem::size_of::<compiler::BlurParams>() as u64,
        );
        ctx.register_compiled_node(CompiledBlur {
            node_id: self.id,
            params: self.params.clone(),
            buffer: passes.params,
            straight_source: source.metadata.alpha_mode == AlphaMode::Unpremultiplied,
        });
        Ok(CompiledOutput::Raster(RasterHandle {
            texture: passes.output,
            domain: source.domain,
            metadata: RasterMetadata {
                alpha_mode: AlphaMode::Premultiplied,
                ..source.metadata
            },
        }))
    }
}
