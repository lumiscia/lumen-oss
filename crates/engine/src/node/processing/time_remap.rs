use crate::node::{NodeId, NodeProperty, PortRef};

use crate::gpu::{BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuFrameBinding};

/// Evaluates a raster input at another frame.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "time_remap", name = "Time Remap", category = "processing")]
pub struct TimeRemap {
    pub id: NodeId,
    /// Source frame to sample.
    #[property(kind = "float", min = 0, step = 1)]
    pub frame: NodeProperty,
    /// Enables looping between the configured loop bounds.
    #[property(kind = "bool")]
    pub loop_enabled: NodeProperty,
    /// First frame in the loop range.
    #[property(kind = "int", min = 0, step = 1)]
    pub loop_start: NodeProperty,
    /// Last frame in the loop range.
    #[property(kind = "int", min = 0, step = 1)]
    pub loop_end: NodeProperty,
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
            frame: NodeProperty::Float(0.0),
            loop_enabled: NodeProperty::Bool(false),
            loop_start: NodeProperty::Int(0),
            loop_end: NodeProperty::Int(0),
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
    frame: &NodeProperty,
    loop_enabled: &NodeProperty,
    loop_start: &NodeProperty,
    loop_end: &NodeProperty,
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
            frame: self.frame.clone(),
            loop_enabled: self.loop_enabled.clone(),
            loop_start: self.loop_start.clone(),
            loop_end: self.loop_end.clone(),
        });
        let target_frame = remap_frame(resolve_settings(
            self.id,
            &self.frame,
            &self.loop_enabled,
            &self.loop_start,
            &self.loop_end,
            &ctx.expr_context(self.id, "frame"),
        )?);
        ctx.with_frame_context(target_frame, |ctx| ctx.compile_port(&self.source))
    }
}

#[derive(Debug, Clone)]
struct TimeRemapFrameBinding {
    node_id: NodeId,
    frame: NodeProperty,
    loop_enabled: NodeProperty,
    loop_start: NodeProperty,
    loop_end: NodeProperty,
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
