use crate::node::{NodeId, NodeProperty, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, FrameBinding, GpuCompileNode, GpuFrameBindNode,
    RasterHandle, compiler,
};

pub(crate) const SHADER: &str = include_str!("hue_saturation.wgsl");

/// Adjusts raster hue, saturation, and lightness.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(
    kind = "hue_saturation",
    name = "Hue Saturation",
    category = "processing"
)]
pub struct HueSaturation {
    pub id: NodeId,
    /// Hue rotation in degrees.
    #[property(kind = "float", name = "Hue", step = 1)]
    pub hue_degrees: NodeProperty,
    /// Saturation multiplier.
    #[property(kind = "float", step = 0.01)]
    pub saturation: NodeProperty,
    /// Lightness offset.
    #[property(kind = "float", step = 0.01)]
    pub lightness: NodeProperty,
    #[input()]
    pub source: PortRef,
}

impl Default for HueSaturation {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            hue_degrees: NodeProperty::Float(0.0),
            saturation: NodeProperty::Float(1.0),
            lightness: NodeProperty::Float(0.0),
            source: PortRef::empty(),
        }
    }
}

impl GpuCompileNode for HueSaturation {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        let (source, texture, params) = ctx.compile_unary_filter(
            self.id,
            &self.source,
            port,
            "hue-saturation",
            SHADER,
            std::mem::size_of::<compiler::HueSaturationParams>() as u64,
        )?;
        ctx.push_frame_binding(FrameBinding::HueSaturation {
            node_id: self.id,
            hue_degrees: self.hue_degrees.clone(),
            saturation: self.saturation.clone(),
            lightness: self.lightness.clone(),
            buffer: params,
        });
        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: source.domain,
            metadata: source.metadata,
        }))
    }
}

impl GpuFrameBindNode for HueSaturation {
    fn bind_gpu_frame(
        &self,
        ctx: &FrameBindContext<'_>,
        binding: &FrameBinding,
        bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        let FrameBinding::HueSaturation {
            node_id,
            hue_degrees,
            saturation,
            lightness,
            buffer,
        } = binding
        else {
            return Ok(());
        };
        let params = compiler::HueSaturationParams {
            hue_offset: (hue_degrees.resolve_float(
                *node_id,
                "hue_degrees",
                &ctx.expr_context(*node_id, "hue_degrees"),
            )? / 360.0) as f32,
            saturation: saturation.resolve_float(
                *node_id,
                "saturation",
                &ctx.expr_context(*node_id, "saturation"),
            )? as f32,
            lightness: lightness.resolve_float(
                *node_id,
                "lightness",
                &ctx.expr_context(*node_id, "lightness"),
            )? as f32,
            _pad: 0.0,
        };
        bound.write_buffer(*buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}
