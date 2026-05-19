use crate::node::{Deferred, NodeId, NodeParams, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuFrameBinding, RasterHandle,
    compiler,
};

pub(crate) const SHADER: &str = include_str!("hue_saturation.wgsl");

/// Adjusts raster hue, saturation, and lightness.
#[derive(Debug, Clone, lumen_macros::NodeParams)]
#[params(evaluated = EvaluatedHueSaturationParams)]
#[cfg_attr(feature = "json", derive(serde::Deserialize), serde(default))]
pub struct HueSaturationParams {
    /// Hue rotation in degrees.
    #[param(kind = "float", name = "Hue", step = 1)]
    pub hue_degrees: Deferred<f64>,
    /// Saturation multiplier.
    #[param(kind = "float", step = 0.01)]
    pub saturation: Deferred<f64>,
    /// Lightness offset.
    #[param(kind = "float", step = 0.01)]
    pub lightness: Deferred<f64>,
}

impl Default for HueSaturationParams {
    fn default() -> Self {
        Self {
            hue_degrees: Deferred::value(0.0),
            saturation: Deferred::value(1.0),
            lightness: Deferred::value(0.0),
        }
    }
}

/// Adjusts raster hue, saturation, and lightness.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(
    kind = "hue_saturation",
    name = "Hue Saturation",
    category = "processing"
)]
pub struct HueSaturation {
    pub id: NodeId,
    #[params]
    pub params: HueSaturationParams,

    #[input()]
    pub source: PortRef,
}

impl Default for HueSaturation {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: HueSaturationParams::default(),
            source: PortRef::empty(),
        }
    }
}

#[derive(Debug, Clone)]
struct HueSaturationFrameBinding {
    node_id: NodeId,
    hue_degrees: Deferred<f64>,
    saturation: Deferred<f64>,
    lightness: Deferred<f64>,
    buffer: lumen_gpu::BufferId,
}

impl GpuFrameBinding for HueSaturationFrameBinding {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let params = compiler::HueSaturationParams {
            hue_offset: (self.hue_degrees.resolve_float(
                self.node_id,
                "hue_degrees",
                &ctx.expr_context(self.node_id, "hue_degrees"),
            )? / 360.0) as f32,
            saturation: self.saturation.resolve_float(
                self.node_id,
                "saturation",
                &ctx.expr_context(self.node_id, "saturation"),
            )? as f32,
            lightness: self.lightness.resolve_float(
                self.node_id,
                "lightness",
                &ctx.expr_context(self.node_id, "lightness"),
            )? as f32,
            _pad: 0.0,
        };
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
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
        ctx.push_frame_binding(HueSaturationFrameBinding {
            node_id: self.id,
            hue_degrees: self.params.hue_degrees.clone(),
            saturation: self.params.saturation.clone(),
            lightness: self.params.lightness.clone(),
            buffer: params,
        });
        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: source.domain,
            metadata: source.metadata,
        }))
    }
}
