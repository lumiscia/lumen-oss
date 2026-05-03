use crate::node::{NodeId, NodeProperty, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, FrameBinding, GpuCompileNode, GpuFrameBindNode,
    RasterHandle, compiler,
};

pub(crate) const SHADER: &str = include_str!("transform.wgsl");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum TransformSampling {
    Nearest = 0,
    Linear = 1,
}

impl TransformSampling {
    pub fn from_int(value: i64) -> Self {
        if value == Self::Nearest as i64 {
            Self::Nearest
        } else {
            Self::Linear
        }
    }
}

#[derive(Debug, Clone, lumen_macros::Node)]
#[node(
    kind = "transform",
    label = "Transform",
    description = "Transforms a raster inside its existing static bounds.",
    category = "processing"
)]
pub struct Transform {
    pub id: NodeId,
    #[property(kind = "float")]
    pub scale_x: NodeProperty,
    #[property(kind = "float")]
    pub scale_y: NodeProperty,
    #[property(kind = "float")]
    pub translate_x: NodeProperty,
    #[property(kind = "float")]
    pub translate_y: NodeProperty,
    #[property(kind = "float")]
    pub rotate: NodeProperty,
    #[property(kind = "float")]
    pub pivot_x: NodeProperty,
    #[property(kind = "float")]
    pub pivot_y: NodeProperty,
    #[property(kind = "int")]
    pub sampling: NodeProperty,
    #[input()]
    pub source: PortRef,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            scale_x: NodeProperty::Float(1.0),
            scale_y: NodeProperty::Float(1.0),
            translate_x: NodeProperty::Float(0.0),
            translate_y: NodeProperty::Float(0.0),
            rotate: NodeProperty::Float(0.0),
            pivot_x: NodeProperty::Float(0.0),
            pivot_y: NodeProperty::Float(0.0),
            sampling: NodeProperty::Int(TransformSampling::Linear as i64),
            source: PortRef::empty(),
        }
    }
}

impl GpuCompileNode for Transform {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        let (source, texture, params) = ctx.compile_unary_filter(
            self.id,
            &self.source,
            port,
            "transform",
            SHADER,
            std::mem::size_of::<compiler::TransformParams>() as u64,
        )?;
        ctx.push_frame_binding(FrameBinding::Transform {
            node_id: self.id,
            scale_x: self.scale_x.clone(),
            scale_y: self.scale_y.clone(),
            translate_x: self.translate_x.clone(),
            translate_y: self.translate_y.clone(),
            rotate: self.rotate.clone(),
            pivot_x: self.pivot_x.clone(),
            pivot_y: self.pivot_y.clone(),
            sampling: self.sampling.clone(),
            buffer: params,
        });
        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: source.domain,
            metadata: source.metadata,
        }))
    }
}

impl GpuFrameBindNode for Transform {
    fn bind_gpu_frame(
        &self,
        ctx: &FrameBindContext<'_>,
        binding: &FrameBinding,
        bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        let FrameBinding::Transform {
            node_id,
            scale_x,
            scale_y,
            translate_x,
            translate_y,
            rotate,
            pivot_x,
            pivot_y,
            sampling,
            buffer,
        } = binding
        else {
            return Ok(());
        };
        let params = compiler::TransformParams {
            scale: [
                scale_x.resolve_float(
                    *node_id,
                    "scale_x",
                    &ctx.expr_context(*node_id, "scale_x"),
                )? as f32,
                scale_y.resolve_float(
                    *node_id,
                    "scale_y",
                    &ctx.expr_context(*node_id, "scale_y"),
                )? as f32,
            ],
            translate: [
                translate_x.resolve_float(
                    *node_id,
                    "translate_x",
                    &ctx.expr_context(*node_id, "translate_x"),
                )? as f32,
                translate_y.resolve_float(
                    *node_id,
                    "translate_y",
                    &ctx.expr_context(*node_id, "translate_y"),
                )? as f32,
            ],
            pivot: [
                pivot_x.resolve_float(
                    *node_id,
                    "pivot_x",
                    &ctx.expr_context(*node_id, "pivot_x"),
                )? as f32,
                pivot_y.resolve_float(
                    *node_id,
                    "pivot_y",
                    &ctx.expr_context(*node_id, "pivot_y"),
                )? as f32,
            ],
            rotate_radians: (rotate.resolve_float(
                *node_id,
                "rotate",
                &ctx.expr_context(*node_id, "rotate"),
            )? as f32)
                .to_radians(),
            sampling: TransformSampling::from_int(sampling.resolve_int(
                *node_id,
                "sampling",
                &ctx.expr_context(*node_id, "sampling"),
            )?) as u32,
            _pad: [0; 4],
        };
        bound.write_buffer(*buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}
