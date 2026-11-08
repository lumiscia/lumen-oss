use bytemuck::Zeroable;

use super::types::{GradientPaint, MAX_GRADIENT_STOPS};

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct GpuPaint {
    pub(crate) colors: [[f32; 4]; MAX_GRADIENT_STOPS],
    pub(crate) offsets: [[f32; 4]; MAX_GRADIENT_STOPS],
    pub(crate) start: [f32; 2],
    pub(crate) end: [f32; 2],
    pub(crate) center: [f32; 2],
    pub(crate) radius: [f32; 2],
    /// The focal point's offset from `center` in radii, inside the unit circle.
    pub(crate) focal_offset: [f32; 2],
    pub(crate) angle: f32,
    pub(crate) kind: u32,
    pub(crate) units: u32,
    pub(crate) spread: u32,
    pub(crate) interpolation: u32,
    pub(crate) stop_count: u32,
    pub(crate) paint_supersample: u32,
    /// The bits of the half width of the stroke a text paint draws along distance field
    /// glyphs, or 0 for every other paint.
    pub(crate) stroke: u32,
    pub(crate) _padding: [u32; 2],
}

impl GpuPaint {
    pub(crate) fn solid(color: [u8; 4]) -> Self {
        let mut paint = Self::zeroed();
        paint.colors[0] = rgba8_to_f32(color);
        paint.offsets[0][0] = 0.0;
        paint.stop_count = 1;
        paint
    }
}

/// Gradient kind, units, spread and interpolation reach the shader as their variant numbers, with
/// kind offset by one because 0 is a solid colour. A gradient without stops is transparent.
pub(crate) fn gradient_to_gpu(gradient: &GradientPaint) -> GpuPaint {
    let mut paint = GpuPaint::solid([0; 4]);
    paint.kind = gradient.kind as u32 + 1;
    paint.units = gradient.units as u32;
    paint.spread = gradient.spread as u32;
    paint.interpolation = gradient.interpolation as u32;
    paint.start = gradient.start;
    paint.end = gradient.end;
    paint.center = gradient.center;
    paint.radius = gradient.radius;
    paint.focal_offset = focal_offset(gradient);
    paint.angle = gradient.angle;
    for (index, stop) in gradient.stops.iter().take(MAX_GRADIENT_STOPS).enumerate() {
        paint.offsets[index][0] = stop.offset.clamp(0.0, 1.0);
        paint.colors[index] = rgba8_to_f32(stop.color);
        paint.stop_count = index as u32 + 1;
    }
    paint
}

/// The focal point's offset from the center in radii, moved to `MAX_FOCAL_DISTANCE` of the way to
/// the circle when it lies further out, so the shader's solve for `t` stays well defined.
fn focal_offset(gradient: &GradientPaint) -> [f32; 2] {
    const MAX_FOCAL_DISTANCE: f32 = 0.99;
    let Some(focal) = gradient.focal else {
        return [0.0, 0.0];
    };
    let radius = gradient.radius.map(|axis| axis.max(0.00001));
    let offset = [
        (focal[0] - gradient.center[0]) / radius[0],
        (focal[1] - gradient.center[1]) / radius[1],
    ];
    let distance = offset[0].hypot(offset[1]);
    let scale = if distance > MAX_FOCAL_DISTANCE {
        MAX_FOCAL_DISTANCE / distance
    } else {
        1.0
    };
    offset.map(|axis| axis * scale)
}

fn rgba8_to_f32(color: [u8; 4]) -> [f32; 4] {
    [
        f32::from(color[0]) / 255.0,
        f32::from(color[1]) / 255.0,
        f32::from(color[2]) / 255.0,
        f32::from(color[3]) / 255.0,
    ]
}
