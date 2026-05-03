use crate::node::{NodeId, NodeProperty, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, FrameBinding, GpuCompileNode, GpuFrameBindNode,
};

#[derive(Debug, Clone, lumen_macros::Node)]
#[node(
    kind = "time_remap",
    label = "Time Remap",
    description = "Evaluates a raster input at another frame.",
    category = "processing"
)]
pub struct TimeRemap {
    pub id: NodeId,
    #[property(kind = "float")]
    pub frame: NodeProperty,
    #[property(kind = "bool")]
    pub loop_enabled: NodeProperty,
    #[property(kind = "int")]
    pub loop_start: NodeProperty,
    #[property(kind = "int")]
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

impl GpuCompileNode for TimeRemap {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        if port.port != "output" {
            return Err(ctx.missing_output(self.id, &port.port));
        }
        let source = ctx.compile_port(&self.source)?;
        ctx.push_frame_binding(FrameBinding::TimeRemap {
            node_id: self.id,
            frame: self.frame.clone(),
            loop_enabled: self.loop_enabled.clone(),
            loop_start: self.loop_start.clone(),
            loop_end: self.loop_end.clone(),
        });
        Ok(source)
    }
}

impl GpuFrameBindNode for TimeRemap {
    fn bind_gpu_frame(
        &self,
        ctx: &FrameBindContext<'_>,
        binding: &FrameBinding,
        _bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        let FrameBinding::TimeRemap {
            node_id,
            frame,
            loop_enabled,
            loop_start,
            loop_end,
        } = binding
        else {
            return Ok(());
        };
        let _ = remap_frame(TimeRemapSettings {
            frame: frame.resolve_float(*node_id, "frame", &ctx.expr_context(*node_id, "frame"))?,
            loop_enabled: loop_enabled.resolve_bool(
                *node_id,
                "loop_enabled",
                &ctx.expr_context(*node_id, "loop_enabled"),
            )?,
            loop_start: loop_start.resolve_int(
                *node_id,
                "loop_start",
                &ctx.expr_context(*node_id, "loop_start"),
            )?,
            loop_end: loop_end.resolve_int(
                *node_id,
                "loop_end",
                &ctx.expr_context(*node_id, "loop_end"),
            )?,
        });
        Ok(())
    }
}
