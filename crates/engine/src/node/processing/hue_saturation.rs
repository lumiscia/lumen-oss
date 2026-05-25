use crate::node::{NodeId, NodeParamEvalContext, NodeParams, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode, RasterHandle,
    compiler,
};

pub(crate) const SHADER: &str = include_str!("hue_saturation.wgsl");

/// Adjusts raster hue, saturation, and lightness.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct HueSaturationParams {
    /// Hue rotation in degrees.
    #[meta(name = "Hue", step = 1)]
    pub hue_degrees: f64,
    /// Saturation multiplier.
    #[meta(step = 0.01)]
    pub saturation: f64,
    /// Lightness offset.
    #[meta(step = 0.01)]
    pub lightness: f64,
}

impl Default for HueSaturationParams {
    fn default() -> Self {
        Self {
            hue_degrees: 0.0,
            saturation: 1.0,
            lightness: 0.0,
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
    pub params: HueSaturationParamsDelegate,

    #[input()]
    pub source: PortRef,
}

impl Default for HueSaturation {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: HueSaturationParamsDelegate::default(),
            source: PortRef::empty(),
        }
    }
}

#[derive(Debug, Clone)]
struct CompiledHueSaturation {
    node_id: NodeId,
    params: HueSaturationParamsDelegate,
    buffer: lumen_gpu::BufferId,
}

impl GpuCompiledNode for CompiledHueSaturation {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let params = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(self.node_id, "params"),
        })?;
        let gpu_params = compiler::HueSaturationParams {
            hue_offset: (params.hue_degrees / 360.0) as f32,
            saturation: params.saturation as f32,
            lightness: params.lightness as f32,
            _pad: 0.0,
        };
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&gpu_params));
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
        ctx.register_compiled_node(CompiledHueSaturation {
            node_id: self.id,
            params: self.params.clone(),
            buffer: params,
        });
        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: source.domain,
            metadata: source.metadata,
        }))
    }
}
