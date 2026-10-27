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

/// Returns the (possibly fractional) source frame. Consumers that index media floor it themselves.
///
/// Looping runs on the whole frame, exactly as for integer frames, and the fraction is carried
/// separately so rounding can never cross into the next frame or the excluded loop end.
pub fn remap_frame(settings: TimeRemapSettings) -> f64 {
    let frame = settings.frame.max(0.0);
    let mut whole = frame.floor() as i64;
    let fraction = if frame.is_finite() {
        frame - frame.floor()
    } else {
        0.0
    };
    if settings.loop_enabled && settings.loop_end > settings.loop_start {
        let span = settings.loop_end - settings.loop_start;
        whole = settings.loop_start + (whole - settings.loop_start).rem_euclid(span);
    }
    // Matches the integer-frame behaviour, including its wrapping cast.
    let whole = f64::from(whole.max(0) as u32);
    (whole + fraction).min((whole + 1.0).next_down())
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
        ctx.with_frame_context(self.id, target_frame, |ctx| {
            ctx.compile_bounded_port(&self.source)
        })
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

#[cfg(test)]
mod tests {
    use super::{TimeRemapSettings, remap_frame};

    fn remap(frame: f64, loop_enabled: bool) -> f64 {
        remap_frame(TimeRemapSettings {
            frame,
            loop_enabled,
            loop_start: 2,
            loop_end: 6,
        })
    }

    #[test]
    fn keeps_fractional_target_frames() {
        assert_eq!(remap(2.5, false), 2.5);
        assert_eq!(remap(-1.5, false), 0.0);
        assert_eq!(remap(7.25, true), 3.25);
        assert_eq!(remap(1.5, true), 5.5);
    }

    #[test]
    fn integer_target_frames_are_unchanged() {
        assert_eq!(remap(9.0, false), 9.0);
        assert_eq!(remap(6.0, true), 2.0);
        assert_eq!(remap(11.0, true), 3.0);
    }

    #[test]
    fn loop_boundaries_survive_float_rounding() {
        let settings = |frame| TimeRemapSettings {
            frame,
            loop_enabled: true,
            loop_start: 1,
            loop_end: 2,
        };
        assert_eq!(remap_frame(settings(1.0 / 49.0 * 49.0)).floor(), 1.0);
        assert!(remap_frame(settings(1.9999999999999998)) < 2.0);
    }

    #[test]
    fn large_targets_loop_before_clamping() {
        let frame = remap_frame(TimeRemapSettings {
            frame: 4_294_967_296.0,
            loop_enabled: true,
            loop_start: 0,
            loop_end: 10,
        });
        assert_eq!(frame, 6.0);
    }

    #[test]
    fn non_finite_and_huge_targets_match_integer_frames() {
        let settings = |frame, loop_enabled| TimeRemapSettings {
            frame,
            loop_enabled,
            loop_start: 0,
            loop_end: 10,
        };
        assert_eq!(remap_frame(settings(f64::INFINITY, true)), 7.0);
        assert_eq!(remap_frame(settings(4_294_967_296.0, false)), 0.0);
    }
}
