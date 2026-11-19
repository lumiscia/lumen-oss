//! The render scale: device pixels per canvas pixel.

use lumen_gpu::Size;

use crate::composition::Composition;
use crate::error::RenderError;

/// The size of `composition`'s output at the render `scale`, as
/// [`GpuCompositionRenderer::set_render_scale`](super::GpuCompositionRenderer::set_render_scale)
/// takes it. Fails unless `scale` is finite and above 0.
pub fn output_size(composition: &Composition, scale: f32) -> crate::Result<Size> {
    let settings = composition.render_settings();
    Ok(device_size(
        settings.width,
        settings.height,
        validate(scale)?,
    ))
}

/// `scale` as the decimal it was written as, so that a scale of `0.3` makes a 100-pixel canvas
/// 30 device pixels rather than the 31 that `0.3_f32` would round up to. A scale that isn't finite
/// and positive is the host's misuse, `render.internal`.
pub(crate) fn validate(scale: f32) -> crate::Result<f64> {
    if !(scale.is_finite() && scale > 0.0) {
        return Err(RenderError::Internal {
            details: format!("render scale must be a finite number above 0, got {scale}"),
        }
        .into());
    }
    if scale == 1.0 {
        return Ok(1.0);
    }
    Ok(scale.to_string().parse().unwrap_or(f64::from(scale)))
}

/// The device pixels a `logical` extent of at least one canvas pixel covers at `scale`: rounded up,
/// so the last device pixel may cover only part of a canvas pixel.
pub(crate) fn device_extent(logical: u32, scale: f64) -> u32 {
    let logical = logical.max(1);
    if scale == 1.0 {
        return logical;
    }
    // The factor absorbs the error of the product, which is far below any fraction of a pixel a
    // decimal scale can leave.
    (f64::from(logical) * scale * (1.0 - 1e-12))
        .ceil()
        .clamp(1.0, f64::from(u32::MAX)) as u32
}

/// [`device_extent`] of each side.
pub(crate) fn device_size(width: u32, height: u32, scale: f64) -> Size {
    Size::new(device_extent(width, scale), device_extent(height, scale))
}

/// A node's explicit `width` or `height` in device pixels, where 0 still means the canvas.
pub(crate) fn explicit_extent(logical: u32, scale: f64) -> u32 {
    if logical == 0 {
        0
    } else {
        device_extent(logical, scale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_are_finite_and_positive_and_sizes_round_up() {
        for invalid in [0.0, -0.5, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(validate(invalid).is_err(), "{invalid}");
        }
        assert_eq!(validate(1.0).unwrap(), 1.0);
        assert_eq!(validate(0.3).unwrap(), 0.3);
        assert_eq!(validate(2.5).unwrap(), 2.5);

        let size = |width, height, scale: f32| device_size(width, height, validate(scale).unwrap());
        assert_eq!(size(1920, 1080, 1.0), Size::new(1920, 1080));
        assert_eq!(size(101, 51, 0.5), Size::new(51, 26));
        assert_eq!(size(100, 100, 0.3), Size::new(30, 30));
        assert_eq!(size(3, 3, 0.01), Size::new(1, 1));
        assert_eq!(size(10, 7, 1.5), Size::new(15, 11));
        assert_eq!(explicit_extent(0, 0.5), 0);
        assert_eq!(explicit_extent(9, 0.5), 5);
    }
}
