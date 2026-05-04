use crate::node::{NodeId, NodeProperty, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, FrameBinding, GpuCompileNode, GpuFrameBindNode,
    RasterHandle, compiler,
};

pub(crate) const SHADER: &str = include_str!("wgsl_shader.wgsl");

#[derive(Debug, Clone, lumen_macros::Node)]
#[node(
    kind = "wgsl_shader",
    label = "WGSL Shader",
    description = "Runs a custom WGSL compute shader over a raster.",
    category = "processing"
)]
pub struct WgslShader {
    pub id: NodeId,
    #[property(kind = "string")]
    pub shader: NodeProperty,
    #[property(kind = "float")]
    pub value0: NodeProperty,
    #[property(kind = "float")]
    pub value1: NodeProperty,
    #[property(kind = "float")]
    pub value2: NodeProperty,
    #[property(kind = "float")]
    pub value3: NodeProperty,
    #[input()]
    pub source: PortRef,
}

impl Default for WgslShader {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            shader: NodeProperty::String(String::new()),
            value0: NodeProperty::Float(0.0),
            value1: NodeProperty::Float(0.0),
            value2: NodeProperty::Float(0.0),
            value3: NodeProperty::Float(0.0),
            source: PortRef::empty(),
        }
    }
}

impl GpuCompileNode for WgslShader {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        let shader =
            self.shader
                .resolve_string(self.id, "shader", &ctx.expr_context(self.id, "shader"))?;
        let shader = if shader.trim().is_empty() {
            SHADER
        } else {
            shader.as_str()
        };
        let (source, texture, params) = ctx.compile_unary_filter(
            self.id,
            &self.source,
            port,
            "wgsl-shader",
            shader,
            std::mem::size_of::<compiler::WgslShaderParams>() as u64,
        )?;
        ctx.push_frame_binding(FrameBinding::WgslShader {
            node_id: self.id,
            value0: self.value0.clone(),
            value1: self.value1.clone(),
            value2: self.value2.clone(),
            value3: self.value3.clone(),
            buffer: params,
        });
        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: source.domain,
            metadata: source.metadata,
        }))
    }
}

impl GpuFrameBindNode for WgslShader {
    fn bind_gpu_frame(
        &self,
        ctx: &FrameBindContext<'_>,
        binding: &FrameBinding,
        bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        let FrameBinding::WgslShader {
            node_id,
            value0,
            value1,
            value2,
            value3,
            buffer,
        } = binding
        else {
            return Ok(());
        };
        let params = compiler::WgslShaderParams {
            values: [
                value0.resolve_float(*node_id, "value0", &ctx.expr_context(*node_id, "value0"))?
                    as f32,
                value1.resolve_float(*node_id, "value1", &ctx.expr_context(*node_id, "value1"))?
                    as f32,
                value2.resolve_float(*node_id, "value2", &ctx.expr_context(*node_id, "value2"))?
                    as f32,
                value3.resolve_float(*node_id, "value3", &ctx.expr_context(*node_id, "value3"))?
                    as f32,
            ],
        };
        bound.write_buffer(*buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}
