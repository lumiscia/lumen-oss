//! Accumulation motion blur: a subgraph rendered at sub-frame times across a shutter and
//! averaged.

use bytemuck::{Pod, Zeroable};
use lumen_gpu::{LayoutEntry, LayoutKind, Res, Size, TextureDesc, TextureDomain};

use crate::error::RenderError;
use crate::expr::ExpressionContext;
use crate::gpu::frame_graph::{
    ComputeProgram, CropParams, FrameCx, Raster, Staticness, dispatch_for, held, quantized_domain,
};
use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams, motion};

/// Most sub-frames a `motion_blur` node renders.
pub(crate) const MAX_MOTION_BLUR_SAMPLES: i64 = 64;

/// Adds a sample to the running sum, which the last sample divides by the number of samples. The
/// sum is kept in 32-bit float, read with `textureLoad`, so a still source comes out unchanged.
pub(crate) const SHADER: &str = include_str!("motion_blur.wgsl");

/// The running sum's format.
pub(crate) const SUM_FORMAT: lumen_gpu::wgpu::TextureFormat =
    lumen_gpu::wgpu::TextureFormat::Rgba32Float;

/// Adds a sample to the running sum.
pub(crate) const ACCUMULATE: ComputeProgram = ComputeProgram {
    label: "motion-blur",
    shader: SHADER,
    entry: "accumulate_main",
    layout: &[
        LayoutEntry::compute(0, LayoutKind::UnfilterableTexture),
        LayoutEntry::compute(1, LayoutKind::Texture),
        LayoutEntry::compute(2, LayoutKind::Uniform),
        LayoutEntry::compute(3, LayoutKind::StorageTexture(SUM_FORMAT)),
    ],
};

/// Adds the last sample and divides the sum by the number of samples.
pub(crate) const RESOLVE: ComputeProgram = ComputeProgram {
    label: "motion-blur",
    shader: SHADER,
    entry: "resolve_main",
    layout: &[
        LayoutEntry::compute(0, LayoutKind::UnfilterableTexture),
        LayoutEntry::compute(1, LayoutKind::Texture),
        LayoutEntry::compute(2, LayoutKind::Uniform),
        LayoutEntry::compute(4, crate::gpu::frame_graph::RECORD_STORAGE_TEXTURE),
    ],
};

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "Params", source = SHADER)]
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
        let params = self.params.eval(&DelegateEvalContext::new(self.id, expr))?;
        let invalid =
            |details: String| Err(RenderError::evaluating::<MotionBlur>(self.id, details).into());
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
        programs.compute(&ACCUMULATE)?;
        programs.compute(&RESOLVE)
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
            Some(offsets) => record_motion_blur(cx, self.id, source, &offsets),
            None => cx.input(source),
        }
    }

    fn magnification(&self) -> f64 {
        1.0
    }

    // A `motion_blur` mixes its source at frames other than this one.
    fn staticness(&self, reads_frame: bool) -> Staticness {
        if reads_frame {
            Staticness::Never
        } else {
            held(reads_frame)
        }
    }
}

/// Records `source` at each of `offsets` frames from this one and averages the samples in
/// linear, premultiplied colour. Each sample is summed into a running target as soon as it is recorded,
/// so its textures go back to the pool for the next. The samples share a path of their own,
/// keeping their text and video state apart from the source's outside the blur.
fn record_motion_blur(
    cx: &mut FrameCx<'_>,
    node_id: NodeId,
    source: NodeId,
    offsets: &[f64],
) -> crate::Result<Raster> {
    cx.record_samples(node_id, |cx| accumulate(cx, source, offsets))
}

fn accumulate(cx: &mut FrameCx<'_>, source: NodeId, offsets: &[f64]) -> crate::Result<Raster> {
    let frame = cx.frame_time();
    let mut sum: Option<Raster> = None;
    // The domain and bounds every sample had, which the average keeps when they all agree.
    let mut shared: Option<Option<(TextureDomain, Option<CropParams>)>> = None;
    for (index, offset) in offsets.iter().enumerate() {
        let sample = cx.input_sample(source, index as u32, frame.offset(*offset))?;
        let this = (sample.domain, sample.bounds);
        shared = Some(match shared {
            None => Some(this),
            Some(kept) => kept.filter(|kept| *kept == this),
        });
        let last = index + 1 == offsets.len();
        // A blank sample adds nothing, so the sum starts at the first sample that draws, and
        // later blank samples count only towards the divisor.
        let blank = cx.is_transparent(&sample);
        if blank && !last {
            continue;
        }
        if blank && sum.is_none() {
            return Ok(cx.transparent(sample.domain.display_size()));
        }
        sum = Some(add_sample(
            cx,
            sum,
            sample,
            blank,
            last,
            offsets.len() as u32,
        )?);
    }
    let mut average = sum.expect("motion blur takes at least two samples");
    if let Some(Some((domain, bounds))) = shared
        && domain == average.domain
    {
        average.bounds = bounds;
    }
    Ok(average)
}

/// `sample` added to `sum`, over the union of their domains or over the sum's alone when the
/// sample is `blank`. The sum is `SUM_FORMAT` until the `last` sample, which writes the average of
/// all `samples` to the working space.
fn add_sample(
    cx: &mut FrameCx<'_>,
    sum: Option<Raster>,
    sample: Raster,
    blank: bool,
    last: bool,
    samples: u32,
) -> crate::Result<Raster> {
    let (display, data) = match sum {
        Some(sum) if blank => (sum.domain.display_size(), sum.domain.data_rect),
        Some(sum) => {
            let [held, shown] = [sum, sample].map(|raster| raster.domain.display_size());
            (
                Size::new(held.width.max(shown.width), held.height.max(shown.height)),
                sum.domain.data_rect.union(sample.domain.data_rect),
            )
        }
        None => (sample.domain.display_size(), sample.domain.data_rect),
    };
    let domain = quantized_domain(display, data);
    let origin = [domain.data_rect.x, domain.data_rect.y];
    let from = |raster: &Raster| {
        let at = raster.origin();
        [origin[0] - at[0], origin[1] - at[1]]
    };
    let params = AccumulateParams {
        sum_offset: sum.as_ref().map_or([0, 0], from),
        sample_offset: from(&sample),
        has_sum: u32::from(sum.is_some()),
        samples,
        _pad: [0; 2],
    };
    let sum_tex = match sum {
        Some(sum) => sum.tex,
        None => cx.unused_float(),
    };
    let size = domain.storage_size;
    let (program, binding, tex) = if last {
        (&RESOLVE, 4, cx.storage_target(size))
    } else {
        let usage = TextureDesc::storage(size, SUM_FORMAT).usage;
        let tex = cx.rec.acquire(size, SUM_FORMAT, usage);
        (&ACCUMULATE, 3, tex)
    };
    let program = cx.program(program)?;
    let uniform = cx.rec.uniform(&params);
    cx.rec.compute(
        program,
        &[
            (0, Res::Tex(sum_tex)),
            (1, Res::Tex(sample.tex)),
            (2, uniform),
            (binding, Res::Tex(tex)),
        ],
        dispatch_for(size),
    );
    Ok(Raster {
        tex,
        domain,
        bounds: None,
        reduced: None,
    })
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
