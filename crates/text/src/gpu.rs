use bytemuck::{Pod, Zeroable};

pub const GLYPH_MODE_RASTER_MASK: u32 = 0;
pub const GLYPH_MODE_RASTER_COLOR: u32 = 1;
pub const GLYPH_MODE_MSDF: u32 = 2;
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
    /// Box that relative paint coordinates map to `0..1`, as `[x, y, width, height]`.
    pub paint_bounds: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct GpuGlyphInstance {
    pub rect: [f32; 4],
    /// Atlas texel rectangle as `[x0, y0, x1, y1]`.
    pub uv_rect: [f32; 4],
    pub color: [f32; 4],
    pub mode: u32,
    pub flags: u32,
    /// The glyph's own transform, applied to `rect` before the text's: the offset after
    /// the columns of `linear`. Used only with [`GLYPH_TRANSFORMED`].
    pub offset: [f32; 2],
    pub linear: [[f32; 2]; 2],
}
