use bytemuck::{Pod, Zeroable};

use crate::gpu::frame_graph::{ComputeProgram, Raster};
use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams, OutputRef};

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("../../gpu/shaders/color.wgsl"),
    include_str!("pixel_sort.wgsl")
);
const PIXEL_SORT: ComputeProgram = ComputeProgram::spatial("pixel-sort", SHADER);

/// Most device pixels one sorted segment may hold: the keys of a segment sit in workgroup memory.
const MAX_SEGMENT: u32 = 1024;
/// Keys one workgroup sorts at once, as many whole segments as fit.
const WORKGROUP_SLOTS: u32 = 1024;

/// The axis runs of pixels are taken along.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum", expressions)]
pub enum SortDirection {
    /// Along each row, left to right.
    #[default]
    Horizontal = 0,
    /// Down each column, top to bottom.
    Vertical = 1,
}

/// The value pixels are sorted and selected by.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum", expressions)]
pub enum SortKey {
    /// The Rec. 709 luma of the sRGB-encoded colour.
    #[default]
    Luma = 0,
    /// The hue of the sRGB-encoded colour, 0 to 1 around the colour wheel from red. Greys are 0.
    Hue = 1,
    /// The saturation of the sRGB-encoded colour, the span of its channels over its largest.
    Saturation = 2,
}

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct PixelSortParams {
    /// The direction runs of pixels are taken along.
    #[meta()]
    pub direction: SortDirection,
    /// Sorts from the highest `key` to the lowest instead.
    #[meta()]
    pub reverse: bool,
    /// The value that selects pixels and orders them.
    #[meta()]
    pub key: SortKey,
    /// The lowest `key`, 0 to 1, a pixel may have and still be sorted.
    #[meta(min = 0, max = 1, step = 0.01)]
    pub low: f64,
    /// The highest `key`, 0 to 1, a pixel may have and still be sorted.
    #[meta(min = 0, max = 1, step = 0.01)]
    pub high: f64,
    /// The most pixels one run holds, 2 and up. Longer runs are cut at multiples of it from the
    /// canvas origin, and each piece is sorted alone. At most 1024 device pixels: a larger value,
    /// or one that a render scale takes past that, counts as 1024.
    #[meta(min = 2, step = 1, pixels)]
    pub max_length: f64,
}

impl Default for PixelSortParams {
    fn default() -> Self {
        Self {
            direction: SortDirection::Horizontal,
            reverse: false,
            key: SortKey::Luma,
            low: 0.25,
            high: 0.8,
            max_length: 256.0,
        }
    }
}

/// Sorts runs of pixels along rows or columns by brightness, hue or saturation, which smears a
/// picture into streaks.
///
/// Only pixels whose `key` lies from `low` to `high` are sorted. A run is a span of those pixels
/// along the `direction`; pixels outside the band, and fully transparent ones, stay where they
/// are and end the run. A run longer than `max_length` is cut into pieces at multiples of
/// `max_length` pixels from the canvas origin. Each run is sorted from the lowest `key` to the
/// highest, or the reverse, and pixels with equal keys keep their order. A pixel moves whole, its
/// alpha with its colour, so the output holds exactly the pixels of the source.
///
/// For example, `{ "key": "luma", "low": 0.2, "high": 0.9, "max_length": 120 }` drags the lit
/// parts of a photo into horizontal streaks up to 120 pixels long.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "pixel_sort", name = "Pixel Sort", category = "processing")]
pub struct PixelSort {
    pub id: NodeId,
    #[params]
    pub params: PixelSortParamsDelegate,
    #[input()]
    pub source: Option<OutputRef>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "PixelSortParams", source = SHADER)]
struct PixelSortUniforms {
    /// The length of a line in device pixels, the canvas position of its first texel, the segment
    /// length, and the segment length rounded up to a power of two.
    line: [i32; 4],
    /// The first segment of the line, the segments a workgroup sorts, and 0, 0.
    segments: [i32; 4],
    /// Whether the sort is vertical and whether it is reversed, each 0 or 1, then the key.
    mode: [u32; 4],
    /// The lowest and highest key that is sorted.
    band: [f32; 4],
}

impl crate::gpu::frame_graph::RecordNode for PixelSort {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.compute(&PIXEL_SORT)
    }

    fn record(&self, cx: &mut crate::gpu::frame_graph::FrameCx<'_>) -> crate::Result<Raster> {
        let source = cx.required(self.id, "source", self.source)?;
        let params = self.params.eval(&DelegateEvalContext {
            node_id: self.id,
            expr: &cx.expr(),
            pixel_scale: cx.pixel_scale(),
        })?;
        let vertical = params.direction == SortDirection::Vertical;
        let size = source.domain.storage_size;
        let [x, y] = source.origin();
        let (length, lines, origin) = if vertical {
            (size.height, size.width, y)
        } else {
            (size.width, size.height, x)
        };
        let segment = if params.max_length.is_finite() {
            params.max_length.round().clamp(2.0, f64::from(MAX_SEGMENT)) as i32
        } else {
            MAX_SEGMENT as i32
        };
        let padded = segment.unsigned_abs().next_power_of_two();
        let per_group = (WORKGROUP_SLOTS / padded) as i32;
        let first = origin.div_euclid(segment);
        let last = (i64::from(origin) + i64::from(length) - 1).div_euclid(i64::from(segment));
        let count = (last - i64::from(first) + 1) as u32;

        let uniforms = PixelSortUniforms {
            line: [length as i32, origin, segment, padded as i32],
            segments: [first, per_group, 0, 0],
            mode: [
                u32::from(vertical),
                u32::from(params.reverse),
                params.key as u32,
                0,
            ],
            band: [params.low as f32, params.high as f32, 0.0, 0.0],
        };
        let tex = cx.storage_target(size);
        let program = cx.program(&PIXEL_SORT)?;
        let uniform = cx.rec.uniform(&uniforms);
        cx.dispatch(
            program,
            &[
                (0, lumen_gpu::Res::Tex(source.tex)),
                (1, uniform),
                (2, lumen_gpu::Res::Tex(tex)),
            ],
            lumen_gpu::Dispatch {
                x: count.div_ceil(per_group as u32),
                y: lines,
                z: 1,
            },
        );
        Ok(Raster { tex, ..source })
    }

    fn magnification(&self) -> f64 {
        1.0
    }
}
