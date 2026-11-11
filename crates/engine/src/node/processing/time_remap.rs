use crate::node::{NodeId, NodeParams};

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
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "time_remap", name = "Time Remap", category = "processing")]
pub struct TimeRemap {
    pub id: NodeId,
    #[params]
    pub params: TimeRemapParamsDelegate,

    #[input()]
    pub source: Option<NodeId>,
}

impl TimeRemapParams {
    /// The loop's first frame and its length, widened so any `i64` bounds are exact, or `None`
    /// when looping is off or the range is empty.
    pub fn loop_range(&self) -> Option<(i128, i128)> {
        let start = i128::from(self.loop_start);
        let span = i128::from(self.loop_end) - start;
        (self.loop_enabled && span > 0).then_some((start, span))
    }
}

const MAX_FRAME: f64 = 9_007_199_254_740_992.0;

/// Returns the (possibly fractional) source frame. Consumers that index media floor it themselves.
///
/// Looping runs on the whole frame, exactly as for integer frames, and the fraction is carried
/// separately so rounding can never cross into the next frame or the excluded loop end. Frames
/// below zero clamp to zero; a frame that is not finite, or above 2^53 where an `f64` no longer
/// holds every whole frame, is an error.
pub fn remap_frame(node_id: NodeId, params: &TimeRemapParams) -> crate::Result<f64> {
    if !params.frame.is_finite() || params.frame > MAX_FRAME {
        return Err(crate::error::PropertyError::InvalidType {
            node_id,
            property_path: "frame".to_string(),
            expected: "a finite number up to 2^53",
            actual: if params.frame.is_nan() {
                "NaN"
            } else if params.frame.is_infinite() {
                "infinity"
            } else {
                "a larger number"
            },
        }
        .into());
    }
    let frame = params.frame.max(0.0);
    let mut whole = frame.floor() as i128;
    let fraction = frame - frame.floor();
    if let Some((start, span)) = params.loop_range() {
        whole = start + (whole - start).rem_euclid(span);
    }
    let whole = whole.max(0) as f64;
    if fraction == 0.0 {
        return Ok(whole);
    }
    Ok((whole + fraction).min((whole + 1.0).next_down()).max(whole))
}

impl crate::gpu::frame_graph::RecordNode for TimeRemap {
    fn programs(&self, _: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        Ok(())
    }

    fn record(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        cx.record_time_remap(self)
    }
}

#[cfg(test)]
mod tests {
    use super::{TimeRemapParams, remap_frame};
    use crate::node::NodeId;

    fn params(frame: f64, loop_enabled: bool, loop_start: i64, loop_end: i64) -> TimeRemapParams {
        TimeRemapParams {
            frame,
            loop_enabled,
            loop_start,
            loop_end,
        }
    }

    fn remap(frame: f64, loop_enabled: bool) -> f64 {
        remap_frame(NodeId::new(1), &params(frame, loop_enabled, 2, 6)).unwrap()
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
        let node = NodeId::new(1);
        let looped = |frame| remap_frame(node, &params(frame, true, 1, 2)).unwrap();
        assert_eq!(looped(1.0 / 49.0 * 49.0).floor(), 1.0);
        assert!(looped(1.9999999999999998) < 2.0);
    }

    #[test]
    fn numeric_boundaries() {
        let node = NodeId::new(1);
        let remap = |frame, loop_enabled, loop_start, loop_end| {
            remap_frame(node, &params(frame, loop_enabled, loop_start, loop_end))
        };
        assert_eq!(remap(4_294_967_296.0, true, 0, 10).unwrap(), 6.0);
        assert_eq!(
            remap(4_294_967_296.0, false, 0, 10).unwrap(),
            4_294_967_296.0
        );
        assert_eq!(
            remap(9_007_199_254_740_992.0, false, 0, 10).unwrap(),
            9_007_199_254_740_992.0
        );
        assert_eq!(remap(9_007_199_254_740_992.0, true, 0, 10).unwrap(), 2.0);
        assert_eq!(
            remap(0.5, true, 9_007_199_254_740_992, 9_007_199_254_740_994).unwrap(),
            9_007_199_254_740_992.0
        );
        assert!(remap(9_223_372_036_854_775_808.0, true, 0, 10).is_err());
        assert!(remap(1e40, false, 0, 10).is_err());
        assert_eq!(remap(9.0, true, i64::MIN, i64::MAX).unwrap(), 9.0);
        assert!(remap(f64::INFINITY, true, 0, 10).is_err());
        assert!(remap(f64::NAN, false, 0, 10).is_err());
    }
}
