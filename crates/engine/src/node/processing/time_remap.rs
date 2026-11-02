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
/// separately so rounding can never cross into the next frame or the excluded loop end. Frames
/// below zero clamp to zero; a frame that is not finite is an error.
pub fn remap_frame(node_id: NodeId, settings: TimeRemapSettings) -> crate::Result<f64> {
    if !settings.frame.is_finite() {
        return Err(crate::error::PropertyError::InvalidType {
            node_id,
            property_path: "frame".to_string(),
            expected: "a finite number",
            actual: if settings.frame.is_nan() {
                "NaN"
            } else {
                "infinity"
            },
        }
        .into());
    }
    let frame = settings.frame.max(0.0);
    let mut whole = i128::from(frame.floor() as i64);
    let fraction = frame - frame.floor();
    if settings.loop_enabled && settings.loop_end > settings.loop_start {
        let start = i128::from(settings.loop_start);
        let span = i128::from(settings.loop_end) - start;
        whole = start + (whole - start).rem_euclid(span);
    }
    let whole = whole.max(0) as f64;
    Ok((whole + fraction).min((whole + 1.0).next_down()))
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
        ctx.compile_time_remap(self)
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
        remap_frame(
            self.node_id,
            TimeRemapSettings {
                frame: evaluated.frame,
                loop_enabled: evaluated.loop_enabled,
                loop_start: evaluated.loop_start,
                loop_end: evaluated.loop_end,
            },
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{TimeRemapSettings, remap_frame};
    use crate::node::NodeId;

    fn remap(frame: f64, loop_enabled: bool) -> f64 {
        remap_frame(
            NodeId::new(1),
            TimeRemapSettings {
                frame,
                loop_enabled,
                loop_start: 2,
                loop_end: 6,
            },
        )
        .unwrap()
    }

    #[test]
    fn keeps_fractional_target_frames() {
        assert_eq!(remap(2.5, false), 2.5);
        assert_eq!(remap(-1.5, false), 0.0);
        assert_eq!(remap(7.25, true), 3.25);
        assert_eq!(remap(1.5, true), 5.5);
    }

    #[test]
    fn loop_boundaries_survive_float_rounding() {
        let settings = |frame| TimeRemapSettings {
            frame,
            loop_enabled: true,
            loop_start: 1,
            loop_end: 2,
        };
        let node = NodeId::new(1);
        assert_eq!(
            remap_frame(node, settings(1.0 / 49.0 * 49.0))
                .unwrap()
                .floor(),
            1.0
        );
        assert!(remap_frame(node, settings(1.9999999999999998)).unwrap() < 2.0);
    }

    #[test]
    fn numeric_boundaries() {
        let node = NodeId::new(1);
        let settings = |frame, loop_enabled, loop_start, loop_end| TimeRemapSettings {
            frame,
            loop_enabled,
            loop_start,
            loop_end,
        };
        let remap = |settings| remap_frame(node, settings);
        assert_eq!(remap(settings(4_294_967_296.0, true, 0, 10)).unwrap(), 6.0);
        assert_eq!(
            remap(settings(4_294_967_296.0, false, 0, 10)).unwrap(),
            4_294_967_296.0
        );
        assert!(remap(settings(1e30, true, i64::MIN, i64::MAX)).is_ok());
        assert_eq!(remap(settings(9.0, true, i64::MIN, i64::MAX)).unwrap(), 9.0);
        assert!(remap(settings(f64::INFINITY, true, 0, 10)).is_err());
        assert!(remap(settings(f64::NAN, false, 0, 10)).is_err());
    }
}
