//! Accumulation motion blur: a subgraph rendered at sub-frame times across a shutter and
//! averaged.

use bytemuck::{Pod, Zeroable};
use lumen_gpu::{LayoutEntry, LayoutKind};

use crate::error::RenderError;
use crate::expr::ExpressionContext;
use crate::node::{DelegateEvalContext, DelegateEvaluable, JsonNode, NodeId, NodeParams, motion};

/// Most sub-frames a `motion_blur` node renders.
pub(crate) const MAX_MOTION_BLUR_SAMPLES: i64 = 64;

/// Adds a sample to the running sum, which the last sample divides by the number of samples. The
/// sum is kept in 32-bit float, read with `textureLoad`, so a still source comes out unchanged.
pub(crate) const SHADER: &str = include_str!("motion_blur.wgsl");

/// The running sum's format.
pub(crate) const SUM_FORMAT: lumen_gpu::wgpu::TextureFormat =
    lumen_gpu::wgpu::TextureFormat::Rgba32Float;

pub(crate) const ACCUMULATE_LAYOUT: &[LayoutEntry] = &[
    LayoutEntry::compute(0, LayoutKind::UnfilterableTexture),
    LayoutEntry::compute(1, LayoutKind::Texture),
    LayoutEntry::compute(2, LayoutKind::Uniform),
    LayoutEntry::compute(3, LayoutKind::StorageTexture(SUM_FORMAT)),
];

pub(crate) const RESOLVE_LAYOUT: &[LayoutEntry] = &[
    LayoutEntry::compute(0, LayoutKind::UnfilterableTexture),
    LayoutEntry::compute(1, LayoutKind::Texture),
    LayoutEntry::compute(2, LayoutKind::Uniform),
    LayoutEntry::compute(4, crate::gpu::frame_graph::RECORD_STORAGE_TEXTURE),
];

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct AccumulateParams {
    pub(crate) sum_offset: [i32; 2],
    pub(crate) sample_offset: [i32; 2],
    pub(crate) has_sum: u32,
    pub(crate) samples: u32,
    pub(crate) _pad: [u32; 2],
}

/// Renders the source at sub-frame times across a shutter and averages them.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct MotionBlurParams {
    /// Sub-frames rendered across the shutter, from 1 to 64. Each renders the whole source
    /// again, so the cost grows with it. 1 renders the source once, at the frame.
    #[meta(min = 1, max = 64, step = 1)]
    pub samples: i64,
    /// Shutter angle in degrees, centred on the frame, from 0 to 360. 0 is off, 180 is the
    /// usual film look and 360 smears over the whole frame.
    #[meta(min = 0, max = 360, step = 1)]
    pub shutter: f64,
}

impl Default for MotionBlurParams {
    fn default() -> Self {
        Self {
            samples: 8,
            shutter: 180.0,
        }
    }
}

/// Blurs everything that moves in its source, including what analytic motion blur can't see:
/// animated sizes, path data, motion upstream of a transform and motion inside video. Analytic
/// motion blur in the source is off while it samples, and so is a `motion_blur` inside it.
/// Content that already has its own motion blur, like a video recorded with it, is blurred
/// twice.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "motion_blur", name = "Motion Blur", category = "processing")]
pub struct MotionBlur {
    pub id: NodeId,
    #[params]
    pub params: MotionBlurParamsDelegate,
    #[input()]
    pub source: Option<NodeId>,
}

impl MotionBlur {
    /// The frames from this one that the source is rendered at, or `None` when it renders once
    /// at the frame: with one sample or a closed shutter.
    pub(crate) fn sample_offsets(
        &self,
        expr: &ExpressionContext<'_>,
    ) -> crate::Result<Option<Vec<f64>>> {
        let params = self.params.eval(&DelegateEvalContext {
            node_id: self.id,
            expr,
        })?;
        let invalid = |details: String| {
            Err(RenderError::NodeEvaluation {
                node_id: self.id,
                node_kind: <MotionBlur as JsonNode>::KIND,
                details,
            }
            .into())
        };
        if !(1..=MAX_MOTION_BLUR_SAMPLES).contains(&params.samples) {
            return invalid(format!(
                "`samples` is {}; it must be from 1 to {MAX_MOTION_BLUR_SAMPLES}",
                params.samples
            ));
        }
        if !(0.0..=360.0).contains(&params.shutter) {
            return invalid(format!(
                "`shutter` is {}; it must be from 0 to 360 degrees",
                params.shutter
            ));
        }
        let Some(offset) = motion::shutter_offset(params.shutter) else {
            return Ok(None);
        };
        let samples = params.samples as usize;
        if samples == 1 {
            return Ok(None);
        }
        // At the middle of equal parts of the shutter, as analytic motion blur places its taps.
        Ok(Some(
            (0..samples)
                .map(|sample| ((2 * sample + 1) as f64 / samples as f64 - 1.0) * offset)
                .collect(),
        ))
    }
}

impl crate::gpu::frame_graph::RecordNode for MotionBlur {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.compute("motion-blur", SHADER, "accumulate_main", ACCUMULATE_LAYOUT)?;
        programs.compute("motion-blur", SHADER, "resolve_main", RESOLVE_LAYOUT)
    }

    fn record(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        let source = crate::node::required_input(self.id, "source", self.source)?;
        let offsets = if cx.analytic_motion_blur() {
            self.sample_offsets(&cx.expr())?
        } else {
            // Inside another `motion_blur` that samples, this one renders once per sample.
            None
        };
        match offsets {
            Some(offsets) => cx.record_motion_blur(self.id, source, &offsets),
            None => cx.input(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::Deferred;

    fn offsets(samples: i64, shutter: f64) -> crate::Result<Option<Vec<f64>>> {
        let mut node = MotionBlur::default();
        node.params.samples = Deferred::value(samples);
        node.params.shutter = Deferred::value(shutter);
        node.sample_offsets(&ExpressionContext::default())
    }

    #[test]
    fn samples_sit_at_the_middle_of_equal_parts_of_a_centred_shutter() {
        assert_eq!(
            offsets(4, 180.0).unwrap(),
            Some(vec![-0.1875, -0.0625, 0.0625, 0.1875])
        );
        assert_eq!(offsets(1, 180.0).unwrap(), None);
        assert_eq!(offsets(8, 0.0).unwrap(), None);
        for (samples, shutter) in [
            (0, 180.0),
            (65, 180.0),
            (8, -1.0),
            (8, 361.0),
            (8, f64::NAN),
        ] {
            let error = offsets(samples, shutter).unwrap_err().to_string();
            assert!(error.contains("must be from"), "{error}");
        }
    }
}
