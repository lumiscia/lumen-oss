use crate::node::{Deferred, NodeId, NodeParams, PortRef};

use crate::gpu::{
    AlphaMode, BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode,
    RasterHandle, RasterMetadata, compiler,
};

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("alpha_premultiply.wgsl")
);

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
    operation: f32,
    buffer: lumen_gpu::BufferId,
}

impl GpuCompiledNode for CompiledAlphaPremultiply {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, _ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let gpu_params = compiler::AlphaPremultiplyParams {
            values: [self.operation, 0.0, 0.0, 0.0],
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
        // The mode decides the output metadata, so it must be known at compile time.
        let Deferred::Value(mode) = &self.params.mode else {
            return Err(crate::error::PropertyError::InvalidType {
                node_id: self.id,
                property_path: "mode".to_string(),
                expected: "constant `premultiply` or `unpremultiply`",
                actual: "Expression",
            }
            .into());
        };
        let operation = compiler::alpha_operation(self.id, mode)?;
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
            operation,
            buffer: params,
        });

        let alpha_mode = if operation >= 0.5 {
            AlphaMode::Unpremultiplied
        } else {
            AlphaMode::Premultiplied
        };
        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: source.domain,
            metadata: RasterMetadata {
                alpha_mode,
                ..source.metadata
            },
        }))
    }
}
