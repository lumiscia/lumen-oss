use bytemuck::{Pod, Zeroable};

use super::drawable::{GpuDrawableMotion, GpuDrawableTransform};

/// Most points a path may flatten to. Points buffers are sized to the
/// flattened point count when the geometry is constant, and to this when animated.
pub(crate) const MAX_PATH_POINTS: usize = 16384;

// Bits of the `flags` field in both uniform structs; keep in sync with `vector_common.wgsl`.
pub(super) const STYLE_FILL_ENABLED: u32 = 1;
pub(super) const STYLE_STROKE_ENABLED: u32 = 2;
const STYLE_EDGE_ANTIALIAS: u32 = 4;

pub(super) fn style_flags(fill: bool, stroke: bool, edge_antialias: bool) -> u32 {
    let mut flags = 0;
    if fill {
        flags |= STYLE_FILL_ENABLED;
    }
    if stroke {
        flags |= STYLE_STROKE_ENABLED;
    }
    if edge_antialias {
        flags |= STYLE_EDGE_ANTIALIAS;
    }
    flags
}

/// The canvas pixel of a domain's first texel.
pub(crate) fn domain_origin(domain: lumen_gpu::TextureDomain) -> [i32; 2] {
    [domain.data_rect.x, domain.data_rect.y]
}

/// Buffers of a source rasterized by the path shader.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PathBuffers {
    pub(crate) params: lumen_gpu::BufferId,
    pub(crate) points: lumen_gpu::BufferId,
    pub(crate) bands: lumen_gpu::BufferId,
    pub(crate) band_capacity: usize,
    /// Canvas pixel of the output's first texel.
    pub(crate) origin: [i32; 2],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct ShapeUniforms {
    pub(crate) fill_paint: super::paint::GpuPaint,
    pub(crate) stroke_paint: super::paint::GpuPaint,
    pub(crate) position: [f32; 2],
    pub(crate) size: [f32; 2],
    pub(crate) border_radius: f32,
    pub(crate) stroke_width: f32,
    pub(crate) geometry_kind: u32,
    pub(crate) flags: u32,
    pub(crate) transform: GpuDrawableTransform,
    /// Canvas-space `[min_x, min_y, max_x, max_y]` outside which nothing is drawn.
    pub(crate) screen_bounds: [f32; 4],
    /// Canvas pixel of the output's first texel.
    pub(crate) origin: [i32; 2],
    pub(crate) _origin_pad: [i32; 2],
    pub(crate) motion: GpuDrawableMotion,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct PathUniforms {
    pub(crate) fill_paint: super::paint::GpuPaint,
    pub(crate) stroke_paint: super::paint::GpuPaint,
    pub(crate) position: [f32; 2],
    pub(crate) bounds_min: [f32; 2],
    pub(crate) bounds_size: [f32; 2],
    pub(crate) stroke_width: f32,
    pub(crate) flags: u32,
    pub(crate) _flags_pad: u32,
    pub(crate) trim_start: f32,
    pub(crate) trim_end: f32,
    pub(crate) _pad: u32,
    pub(crate) transform: GpuDrawableTransform,
    /// Canvas-space `[min_x, min_y, max_x, max_y]` outside which nothing is drawn.
    pub(crate) screen_bounds: [f32; 4],
    /// Untransformed `y`, relative to `position`, where the first band starts.
    pub(crate) band_origin: f32,
    /// Bands per untransformed pixel.
    pub(crate) band_scale: f32,
    pub(crate) band_count: u32,
    /// Untransformed distance beyond which a segment cannot cover a pixel.
    pub(crate) reach: f32,
    /// Canvas pixel of the output's first texel.
    pub(crate) origin: [i32; 2],
    pub(crate) _origin_pad: [i32; 2],
    pub(crate) motion: GpuDrawableMotion,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct PathPoint {
    pub(crate) position: [f32; 2],
    pub(crate) contour_start: u32,
    pub(crate) flags: u32,
    pub(crate) offset: f32,
    pub(crate) _pad: f32,
}
