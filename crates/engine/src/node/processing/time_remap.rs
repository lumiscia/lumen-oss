use crate::node::{Deferred, NodeId, NodeParams, PortRef};

use crate::gpu::{BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuFrameBinding};

/// Evaluates a raster input at another frame.
#[derive(Debug, Clone, lumen_macros::NodeParams)]
#[params(evaluated = EvaluatedTimeRemapParams)]
#[cfg_attr(feature = "json", derive(serde::Deserialize), serde(default))]
pub struct TimeRemapParams {
    /// Source frame to sample.
    #[param(kind = "float", min = 0, step = 1)]
    pub frame: Deferred<f64>,
    /// Enables looping between the configured loop bounds.
    #[param(kind = "bool")]
    pub loop_enabled: Deferred<bool>,
    /// First frame in the loop range.
    #[param(kind = "int", min = 0, step = 1)]
    pub loop_start: Deferred<i64>,
    /// Last frame in the loop range.
    #[param(kind = "int", min = 0, step = 1)]
    pub loop_end: Deferred<i64>,
}

impl Default for TimeRemapParams {
    fn default() -> Self {
        Self {
            frame: Deferred::value(0.0),
            loop_enabled: Deferred::value(false),
            loop_start: Deferred::value(0),
            loop_end: Deferred::value(0),
        }
    }
}

/// Evaluates a raster input at another frame.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "time_remap", name = "Time Remap", category = "processing")]
pub struct TimeRemap {
    pub id: NodeId,
    #[params]
    pub params: TimeRemapParams,

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
            params: TimeRemapParams::default(),
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

pub fn resolve_settings(
    node_id: NodeId,
    frame: &Deferred<f64>,
    loop_enabled: &Deferred<bool>,
    loop_start: &Deferred<i64>,
    loop_end: &Deferred<i64>,
    ctx: &crate::expr::ExpressionContext<'_>,
) -> crate::Result<TimeRemapSettings> {
    Ok(TimeRemapSettings {
        frame: frame.resolve_float(node_id, "frame", ctx)?,
        loop_enabled: loop_enabled.resolve_bool(node_id, "loop_enabled", ctx)?,
        loop_start: loop_start.resolve_int(node_id, "loop_start", ctx)?,
        loop_end: loop_end.resolve_int(node_id, "loop_end", ctx)?,
    })
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
        ctx.push_frame_binding(TimeRemapFrameBinding {
            node_id: self.id,
            frame: self.params.frame.clone(),
            loop_enabled: self.params.loop_enabled.clone(),
            loop_start: self.params.loop_start.clone(),
            loop_end: self.params.loop_end.clone(),
        });
        let target_frame = remap_frame(resolve_settings(
            self.id,
            &self.params.frame,
            &self.params.loop_enabled,
            &self.params.loop_start,
            &self.params.loop_end,
            &ctx.expr_context(self.id, "frame"),
        )?);
        ctx.with_frame_context(target_frame, |ctx| ctx.compile_port(&self.source))
    }
}

#[derive(Debug, Clone)]
struct TimeRemapFrameBinding {
    node_id: NodeId,
    frame: Deferred<f64>,
    loop_enabled: Deferred<bool>,
    loop_start: Deferred<i64>,
    loop_end: Deferred<i64>,
}

impl GpuFrameBinding for TimeRemapFrameBinding {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, _bound: &mut BoundFrame) -> crate::Result<()> {
        let _ = remap_frame(resolve_settings(
            self.node_id,
            &self.frame,
            &self.loop_enabled,
            &self.loop_start,
            &self.loop_end,
            &ctx.expr_context(self.node_id, "frame"),
        )?);
        Ok(())
    }
}
