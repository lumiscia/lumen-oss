use crate::node::{NodeId, NodeParamEvalContext, NodeParams, PortRef};

use crate::gpu::{BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode};

/// Evaluates a raster input at another frame.
#[derive(Debug, Clone, Default, lumen_macros::Delegate)]
pub struct TimeRemapParams {
    /// Source frame to sample.
    #[meta(min = 0, step = 1)]
    pub frame: f64,
    /// Enables looping between the configured loop bounds.
    #[meta()]
    pub loop_enabled: bool,
    /// First frame in the loop range.
    #[meta(min = 0, step = 1)]
    pub loop_start: i64,
    /// Last frame in the loop range.
    #[meta(min = 0, step = 1)]
    pub loop_end: i64,
}

/// Evaluates a raster input at another frame.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "time_remap", name = "Time Remap", category = "processing")]
pub struct TimeRemap {
    pub id: NodeId,
    #[params]
    pub params: TimeRemapParamsDelegate,

    #[input()]
    pub source: PortRef,
}

#[derive(Debug, Clone, Copy)]
pub struct TimeRemapSettings {
    pub frame: f64,
    pub loop_enabled: bool,
    pub loop_start: i64,
    pub loop_end: i64,
}

impl Default for TimeRemap {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: TimeRemapParamsDelegate::default(),
            source: PortRef::empty(),
        }
    }
}

pub fn remap_frame(settings: TimeRemapSettings) -> u32 {
    let mut frame = settings.frame.max(0.0).floor() as i64;
    if settings.loop_enabled && settings.loop_end > settings.loop_start {
        let span = settings.loop_end - settings.loop_start;
        frame = settings.loop_start + (frame - settings.loop_start).rem_euclid(span);
    }
    frame.max(0) as u32
}

impl GpuCompileNode for TimeRemap {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        if port.port != "output" {
            return Err(ctx.missing_output(self.id, &port.port));
        }
        ctx.register_compiled_node(CompiledTimeRemap {
            node_id: self.id,
            params: self.params.clone(),
        });
        let params = self.params.eval(&NodeParamEvalContext {
            node_id: self.id,
            expr: &ctx.expr_context(self.id, "params"),
        })?;
        let target_frame = remap_frame(TimeRemapSettings {
            frame: params.frame,
            loop_enabled: params.loop_enabled,
            loop_start: params.loop_start,
            loop_end: params.loop_end,
        });
        ctx.with_frame_context(target_frame, |ctx| ctx.compile_port(&self.source))
    }
}

#[derive(Debug, Clone)]
struct CompiledTimeRemap {
    node_id: NodeId,
    params: TimeRemapParamsDelegate,
}

impl GpuCompiledNode for CompiledTimeRemap {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, _bound: &mut BoundFrame) -> crate::Result<()> {
        let evaluated = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(self.node_id, "params"),
        })?;
        let _ = remap_frame(TimeRemapSettings {
            frame: evaluated.frame,
            loop_enabled: evaluated.loop_enabled,
            loop_start: evaluated.loop_start,
            loop_end: evaluated.loop_end,
        });
        Ok(())
    }
}
