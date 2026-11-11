//! Filtering for `transform` and `resize` when they show their source smaller than its size, so
//! that a pixel averages the texels it covers instead of taking a bilinear sample of four.
//!
//! A pixel covering `footprint` texels along each of the source's axes averages the light under a
//! box that size, which is bilinear filtering at one texel and varies smoothly from there. Up to
//! [`UNCHAINED_FOOTPRINT`] texels along the narrower axis the box is taken on the source itself,
//! which needs no chain. Past it the box is taken on mip levels, between the two around the
//! fractional `lod` that is asked for, so that nothing steps as a zoom animates.

use lumen_gpu::{Size, TexRef};

use crate::gpu::frame_graph::{FrameCx, Raster};
use crate::gpu::mips::{full_levels, mipped};
use crate::gpu::params::CropParams;
use crate::node::NodeId;

/// The most texels the filter spans along an axis of the level it reads; keep in sync with
/// `MINIFY_MAX_SPAN` in `minify.wgsl`.
const MAX_SPAN: f32 = 16.0;
/// The footprint along the narrower axis up to which the box is taken on the source itself, with
/// at most 5 by 5 texels where the other axis is as narrow, which costs less than building a
/// chain. The level then rises over as many texels again to the one where that axis spans a
/// texel, which is far fewer taps than a box this wide would take on a level.
const UNCHAINED_FOOTPRINT: f32 = 4.0;

/// How a source shown smaller than its size is filtered.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Minification {
    /// Texels of the source one output pixel covers along each of the source's axes.
    pub(crate) footprint: [f32; 2],
    /// The mip level the filter reads, between two whole ones.
    pub(crate) lod: f32,
}

impl Minification {
    /// The filter for output pixels that cover `footprint` texels of a `size` source along its
    /// axes, or `None` when they cover no more than one along either, where bilinear filtering
    /// is the same. Mip levels are read only where `mips` says the source can have them.
    pub(crate) fn new(footprint: [f32; 2], size: Size, mips: bool) -> Option<Self> {
        let widest = footprint[0].max(footprint[1]);
        if widest.is_nan() || widest <= 1.0 {
            return None;
        }
        let lod = if mips { lod(footprint, size) } else { 0.0 };
        Some(Self { footprint, lod })
    }

    /// The levels a chain holds for the filter to read.
    pub(crate) fn levels(self) -> u32 {
        self.lod.ceil() as u32 + 1
    }

    /// Whether a mip chain of a source of `size` fits the device's textures.
    pub(crate) fn mips_fit(cx: &FrameCx<'_>, size: Size) -> bool {
        let max = cx.rec.device().limits().max_texture_dimension_2d;
        size.width <= max && size.height <= max
    }
}

/// The texture the filters read, which holds `levels` mip levels: the source itself where that
/// is one, and otherwise its mip chain, or that of its `region` alone.
pub(crate) fn source(
    cx: &mut FrameCx<'_>,
    node: Option<NodeId>,
    raster: Raster,
    region: Option<CropParams>,
    levels: u32,
) -> crate::Result<TexRef> {
    match node {
        Some(node) if levels > 1 => mipped(cx, node, raster, region, levels),
        _ => Ok(raster.tex),
    }
}

/// The mip level to read a source of `size` at, for output pixels that cover `footprint` texels of
/// it. The wider axis is read from a level where it spans no more than [`MAX_SPAN`]. The narrower
/// keeps its detail down to one texel of the level, which the level reaches from the source's own
/// across [`UNCHAINED_FOOTPRINT`] texels past that footprint, so it is zero up to it.
fn lod(footprint: [f32; 2], size: Size) -> f32 {
    let narrow = usize::from(footprint[0] > footprint[1]);
    let wide = 1 - narrow;
    let rise = ((footprint[narrow] - UNCHAINED_FOOTPRINT) / UNCHAINED_FOOTPRINT).clamp(0.0, 1.0);
    (rise * lod_at(footprint[narrow], size, narrow)).max(lod_at(
        footprint[wide] / MAX_SPAN,
        size,
        wide,
    ))
}

/// The fractional mip level whose texel is `extent` texels of the source along `axis`, going by
/// the sides the levels really have, which round down at each. Zero where a texel of the source
/// is no less than that.
fn lod_at(extent: f32, size: Size, axis: usize) -> f32 {
    if extent.is_nan() || extent <= 1.0 {
        return 0.0;
    }
    let side = [size.width, size.height][axis];
    let top = full_levels(size) - 1;
    let ratio = |level: u32| side as f32 / (side >> level).max(1) as f32;
    let level = (0..=top)
        .rev()
        .find(|&level| ratio(level) <= extent)
        .unwrap_or(0);
    if level == top {
        return level as f32;
    }
    let (from, to) = (ratio(level).ln(), ratio(level + 1).ln());
    if to <= from {
        return level as f32;
    }
    level as f32 + ((extent.ln() - from) / (to - from)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: Size = Size::new(3840, 2160);

    #[test]
    fn a_pixel_covering_one_texel_or_fewer_is_not_filtered() {
        assert_eq!(Minification::new([1.0, 1.0], SIZE, true), None);
        assert_eq!(Minification::new([1.0, 0.5], SIZE, true), None);
        assert_eq!(Minification::new([f32::NAN, 1.0], SIZE, true), None);
        assert!(Minification::new([1.01, 1.0], SIZE, true).is_some());
    }

    #[test]
    fn the_level_grows_continuously_from_zero_at_the_widest_unchained_footprint() {
        let lod = |footprint: f32| Minification::new([footprint; 2], SIZE, true).unwrap().lod;
        let mut last = lod(1.001);
        assert_eq!(last, 0.0);
        for step in 1..2000 {
            let next = lod(1.001 + step as f32 * 0.01);
            assert!(next >= last && next - last < 0.05, "{last} to {next}");
            last = next;
        }
        assert_eq!(lod(UNCHAINED_FOOTPRINT), 0.0);
        assert!(lod(UNCHAINED_FOOTPRINT * 1.01) > 0.0);
        assert_eq!(lod(8.0), 3.0);
        assert_eq!(lod(32.0), 5.0);
        assert_eq!(lod(f32::INFINITY), (full_levels(SIZE) - 1) as f32);
    }

    #[test]
    fn a_chain_is_asked_for_only_where_a_level_past_the_first_is_read() {
        assert_eq!(Minification::new([32.0; 2], SIZE, false).unwrap().lod, 0.0);
        let levels =
            |footprint: [f32; 2]| Minification::new(footprint, SIZE, true).unwrap().levels();
        assert_eq!(levels([1.5; 2]), 1);
        assert_eq!(levels([UNCHAINED_FOOTPRINT; 2]), 1);
        assert_eq!(levels([UNCHAINED_FOOTPRINT + 0.1; 2]), 2);
        assert_eq!(levels([32.0; 2]), 6);
    }

    #[test]
    fn a_narrow_axis_is_not_blurred_to_spare_the_wide_one() {
        let lod = |footprint: [f32; 2]| Minification::new(footprint, SIZE, true).unwrap().lod;
        // Down to 16 texels along the wide axis it reads the first level.
        assert_eq!(lod([8.0, 1.0]), 0.0);
        assert_eq!(lod([16.0, 4.0]), 0.0);
        assert_eq!(lod([16.0, 8.0]), 3.0);
        // Past that it must read a coarser one, whatever it costs the narrow axis.
        assert_eq!(lod([64.0, 1.0]), 2.0);
    }

    #[test]
    fn levels_are_judged_by_the_sides_they_really_have() {
        // A 9 by 9 source has levels of 9, 4, 2 and 1 texels across: the second is 2.25 source
        // texels to a texel, so 2 texels are short of it, and read between the first two levels
        // rather than all of the second, which 2.25 reaches.
        let nine = Size::new(9, 9);
        let between = lod_at(2.0, nine, 0);
        assert!(between > 0.8 && between < 0.9, "{between}");
        assert!((lod_at(2.25, nine, 0) - 1.0).abs() < 1e-6);
        assert!((lod_at(4.5, nine, 0) - 2.0).abs() < 1e-6);
    }
}
