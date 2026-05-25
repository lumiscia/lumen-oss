use bytemuck::Zeroable;

use super::types::{
    GradientInterpolation, GradientPaint, GradientSpread, GradientUnits, MAX_GRADIENT_STOPS,
    PaintKind,
};

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct GpuPaint {
    pub(crate) colors: [[f32; 4]; MAX_GRADIENT_STOPS],
    pub(crate) offsets: [[f32; 4]; MAX_GRADIENT_STOPS],
    pub(crate) start: [f32; 2],
    pub(crate) end: [f32; 2],
    pub(crate) center: [f32; 2],
    pub(crate) radius: [f32; 2],
    pub(crate) angle: f32,
    pub(crate) kind: u32,
    pub(crate) units: u32,
    pub(crate) spread: u32,
    pub(crate) interpolation: u32,
    pub(crate) stop_count: u32,
    pub(crate) _pad: [u32; 2],
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

pub(crate) fn gradient_to_gpu(gradient: &GradientPaint, fallback: [u8; 4]) -> GpuPaint {
    let mut paint = GpuPaint::solid(fallback);
    paint.kind = match gradient.kind {
        PaintKind::LinearGradient => 1,
        PaintKind::RadialGradient => 2,
        PaintKind::ConicGradient => 3,
    };
    paint.units = match gradient.units {
        GradientUnits::ObjectBoundingBox => 0,
        GradientUnits::UserSpace => 1,
    };
    paint.spread = match gradient.spread {
        GradientSpread::Pad => 0,
        GradientSpread::Repeat => 1,
        GradientSpread::Reflect => 2,
    };
    paint.interpolation = match gradient.interpolation {
        GradientInterpolation::Srgb => 0,
        GradientInterpolation::LinearSrgb => 1,
    };
    paint.start = gradient.start;
    paint.end = gradient.end;
    paint.center = gradient.center;
    paint.radius = gradient.radius;
    paint.angle = gradient.angle;
    if gradient.stops.len() > MAX_GRADIENT_STOPS {
        tracing::warn!(
            target: "lumen_render",
            stop_count = gradient.stops.len(),
            max_stop_count = MAX_GRADIENT_STOPS,
            "truncating gradient stops for gpu paint"
        );
    }
    for (index, stop) in gradient.stops.iter().take(MAX_GRADIENT_STOPS).enumerate() {
        paint.offsets[index][0] = stop.offset.clamp(0.0, 1.0);
        paint.colors[index] = rgba8_to_f32(stop.color);
        paint.stop_count = index as u32 + 1;
    }
    paint
}

fn rgba8_to_f32(color: [u8; 4]) -> [f32; 4] {
    [
        f32::from(color[0]) / 255.0,
        f32::from(color[1]) / 255.0,
        f32::from(color[2]) / 255.0,
        f32::from(color[3]) / 255.0,
    ]
}

#[cfg(test)]
pub(crate) use gradient_to_gpu as test_gradient_to_gpu;
