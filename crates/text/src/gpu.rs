use bytemuck::{Pod, Zeroable};
use cosmic_text::{SwashContent, SwashImage};

use crate::{AtlasEntry, TextGlyph};
#[cfg(feature = "experimental-msdf")]
use crate::{MSDF_GENERATION_SIZE_PX, MsdfGlyphPlacement};

const GLYPH_MODE_RASTER_MASK: u32 = 0;
const GLYPH_MODE_RASTER_COLOR: u32 = 1;
#[cfg(feature = "experimental-msdf")]
const GLYPH_MODE_MSDF: u32 = 2;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuTextGlobals {
    pub target_size: [f32; 2],
    pub px_range: f32,
    pub glyph_count: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuGlyphInstance {
    pub rect: [f32; 4],
    pub uv_rect: [f32; 4],
    pub color: [f32; 4],
    pub mode: u32,
    pub _padding: [u32; 3],
}

#[cfg(feature = "experimental-msdf")]
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuMsdfGlobals {
    pub atlas_size: [u32; 2],
    pub job_count: u32,
    pub dirty_pixel_count: u32,
    pub _padding: [u32; 2],
}

#[cfg(feature = "experimental-msdf")]
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuMsdfJob {
    pub atlas_rect: [u32; 4],
    pub segment_range: [u32; 2],
    pub pixel_range: [u32; 2],
    pub px_range: f32,
    pub _padding: [u32; 3],
}

#[cfg(feature = "experimental-msdf")]
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuMsdfSegment {
    pub p0: [f32; 2],
    pub p1: [f32; 2],
    pub p2: [f32; 2],
    pub p3: [f32; 2],
    pub kind: u32,
    pub channels: u32,
    pub _padding: [u32; 2],
}

pub fn glyph_instance_for(
    glyph: &TextGlyph,
    atlas_entry: AtlasEntry,
    image: &SwashImage,
) -> GpuGlyphInstance {
    let x = glyph.x + image.placement.left as f32;
    let y = glyph.y - image.placement.top as f32;
    GpuGlyphInstance {
        rect: [
            x,
            y,
            image.placement.width as f32,
            image.placement.height as f32,
        ],
        uv_rect: [
            atlas_entry.uv_min[0],
            atlas_entry.uv_min[1],
            atlas_entry.uv_max[0],
            atlas_entry.uv_max[1],
        ],
        color: glyph.color,
        mode: if image.content == SwashContent::Color {
            GLYPH_MODE_RASTER_COLOR
        } else {
            GLYPH_MODE_RASTER_MASK
        },
        _padding: [0; 3],
    }
}

#[cfg(feature = "experimental-msdf")]
pub fn msdf_glyph_instance_for(
    glyph: &TextGlyph,
    atlas_entry: AtlasEntry,
    placement: &MsdfGlyphPlacement,
) -> GpuGlyphInstance {
    // MSDF outlines are shared across subpixel cache bins, so restore the
    // fractional physical offset that is baked into raster glyph images.
    // The atlas field is generated once at a canonical size and the quad is
    // scaled here, allowing size animation to reuse the same glyph entry.
    let font_size = f32::from_bits(glyph.key.0.font_size_bits).max(1.0);
    let scale = font_size / MSDF_GENERATION_SIZE_PX;
    let x = glyph.x + glyph.key.0.x_bin.as_float() + placement.left * scale;
    let y = glyph.y + glyph.key.0.y_bin.as_float() - placement.top * scale;
    GpuGlyphInstance {
        rect: [
            x,
            y,
            placement.width as f32 * scale,
            placement.height as f32 * scale,
        ],
        uv_rect: [
            atlas_entry.uv_min[0],
            atlas_entry.uv_min[1],
            atlas_entry.uv_max[0],
            atlas_entry.uv_max[1],
        ],
        color: glyph.color,
        mode: GLYPH_MODE_MSDF,
        _padding: [0; 3],
    }
}

#[cfg(feature = "experimental-msdf")]
pub fn is_msdf_instance(instance: &GpuGlyphInstance) -> bool {
    instance.mode == GLYPH_MODE_MSDF
}
