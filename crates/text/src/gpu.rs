use bytemuck::{Pod, Zeroable};

pub const GLYPH_MODE_RASTER_MASK: u32 = 0;
pub const GLYPH_MODE_RASTER_COLOR: u32 = 1;
pub const GLYPH_MODE_MSDF: u32 = 2;

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
    pub _padding: [u32; 3],
}
