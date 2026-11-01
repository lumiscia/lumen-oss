use bytemuck::{Pod, Zeroable};

pub const GLYPH_MODE_RASTER_MASK: u32 = 0;
pub const GLYPH_MODE_RASTER_COLOR: u32 = 1;
pub const GLYPH_MODE_MSDF: u32 = 2;
/// The bits of [`GpuGlyphInstance::mode`] that hold the glyph's `GLYPH_MODE_*` value.
pub const GLYPH_MODE_MASK: u32 = 0xff;
/// The bits of [`GpuGlyphInstance::mode`] from here up hold the index of the glyph's paint.
pub const GLYPH_PAINT_SHIFT: u32 = 8;
/// The number of paints a glyph can index.
pub const GLYPH_MAX_PAINTS: u32 = 1 << (32 - GLYPH_PAINT_SHIFT);
/// Bit of [`GpuGlyphInstance::flags`] set when the glyph has its own transform.
pub const GLYPH_TRANSFORMED: u32 = 1;
/// Bit of [`GpuGlyphInstance::flags`] set on an instance that the preceding lead instance of
/// its cluster draws.
pub const GLYPH_CLUSTER_FOLLOWER: u32 = 2;
/// The bits of [`GpuGlyphInstance::flags`] from here up count the follower instances after a
/// cluster's lead, which draws the cluster's instances as one.
pub const GLYPH_CLUSTER_SHIFT: u32 = 8;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct GpuTextGlobals {
    pub target_size: [f32; 2],
    /// Distance range of MSDF glyphs in atlas texels.
    pub px_range: f32,
    pub glyph_count: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct GpuGlyphInstance {
    pub rect: [f32; 4],
    /// Atlas texel rectangle as `[x0, y0, x1, y1]`.
    pub uv_rect: [f32; 4],
    pub color: [f32; 4],
    /// The glyph's `GLYPH_MODE_*` value in the bits of [`GLYPH_MODE_MASK`], and the index of the
    /// paint it is filled with from [`GLYPH_PAINT_SHIFT`] up. Use [`Self::render_mode`] and
    /// [`Self::paint`] to read them.
    pub mode: u32,
    pub flags: u32,
    /// The glyph's own transform, applied to `rect` before the text's: the offset after
    /// the columns of `linear`. Used only with [`GLYPH_TRANSFORMED`].
    pub offset: [f32; 2],
    pub linear: [[f32; 2]; 2],
}

const _: () = assert!(std::mem::size_of::<GpuGlyphInstance>() == 80);

impl GpuGlyphInstance {
    /// The glyph's `GLYPH_MODE_*` value.
    pub fn render_mode(&self) -> u32 {
        self.mode & GLYPH_MODE_MASK
    }

    /// The index of the paint the glyph is filled with.
    pub fn paint(&self) -> u32 {
        self.mode >> GLYPH_PAINT_SHIFT
    }

    /// Fills the glyph with the paint at `index`, which must be below [`GLYPH_MAX_PAINTS`].
    pub fn set_paint(&mut self, index: u32) {
        debug_assert!(index < GLYPH_MAX_PAINTS);
        self.mode = self.render_mode() | index << GLYPH_PAINT_SHIFT;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_index_leaves_the_render_mode_alone() {
        let mut instance = GpuGlyphInstance::zeroed();
        instance.mode = GLYPH_MODE_MSDF;
        instance.set_paint(GLYPH_MAX_PAINTS - 1);
        assert_eq!(instance.render_mode(), GLYPH_MODE_MSDF);
        assert_eq!(instance.paint(), GLYPH_MAX_PAINTS - 1);
        instance.set_paint(3);
        assert_eq!(
            (instance.render_mode(), instance.paint()),
            (GLYPH_MODE_MSDF, 3)
        );
    }
}
