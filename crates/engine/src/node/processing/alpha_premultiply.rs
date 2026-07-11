use crate::node::{Deferred, NodeId, NodeParamEvalContext, NodeParams, PortRef};

use crate::gpu::{
    AlphaMode, BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode,
    RasterHandle, compiler,
};

pub(crate) const SHADER: &str = include_str!("alpha_premultiply.wgsl");

/// Converts raster alpha between premultiplied and unpremultiplied representations.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct AlphaPremultiplyParams {
    /// Alpha conversion mode.
    #[meta(format = "alpha_premultiply_mode")]
    pub mode: String,
}

impl Default for AlphaPremultiplyParams {
    fn default() -> Self {
        Self {
            mode: "premultiply".to_string(),
        }
    }
}

/// Converts raster alpha between premultiplied and unpremultiplied representations.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(
    kind = "alpha_premultiply",
    name = "Alpha Premultiply",
    category = "processing"
)]
pub struct AlphaPremultiply {
    pub id: NodeId,
    #[params]
    pub params: AlphaPremultiplyParamsDelegate,

    #[input()]
    pub source: PortRef,
}

impl Default for AlphaPremultiply {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: AlphaPremultiplyParamsDelegate::default(),
            source: PortRef::empty(),
        }
    }
}

#[derive(Debug, Clone)]
struct CompiledAlphaPremultiply {
    node_id: NodeId,
    params: AlphaPremultiplyParamsDelegate,
    buffer: lumen_gpu::BufferId,
}

impl GpuCompiledNode for CompiledAlphaPremultiply {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let evaluated = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(self.node_id, "params"),
        })?;
        let gpu_params = compiler::AlphaPremultiplyParams {
            values: [
                compiler::alpha_operation(self.node_id, &evaluated.mode)?,
                0.0,
                0.0,
                0.0,
            ],
        };
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&gpu_params));
        Ok(())
    }
}

impl GpuCompileNode for AlphaPremultiply {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        let metadata = match &self.params.mode {
            Deferred::Value(mode) if compiler::alpha_operation(self.id, mode)? < 0.5 => {
                Some(AlphaMode::Premultiplied)
            }
            Deferred::Value(mode) if compiler::alpha_operation(self.id, mode)? >= 0.5 => {
                Some(AlphaMode::Unpremultiplied)
            }
            _ => None,
        };
        let (source, texture, params) = ctx.compile_unary_filter(
            self.id,
            &self.source,
            port,
            "alpha-premultiply",
            SHADER,
            std::mem::size_of::<compiler::AlphaPremultiplyParams>() as u64,
            None,
        )?;
        ctx.register_compiled_node(CompiledAlphaPremultiply {
            node_id: self.id,
            params: self.params.clone(),
            buffer: params,
        });

        let mut output_metadata = source.metadata;
        if let Some(alpha_mode) = metadata {
            output_metadata.alpha_mode = alpha_mode;
        }
        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: source.domain,
            metadata: output_metadata,
        }))
    }
}
