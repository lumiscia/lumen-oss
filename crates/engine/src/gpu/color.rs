//! The working colour space and the sRGB transfer function on the CPU, matching `color.wgsl`.

/// The format of every raster the compositor writes: linear light with BT.709 primaries and
/// premultiplied alpha, where 1.0 is SDR white and colour may go past it.
pub const WORKING_COLOR_FORMAT: lumen_gpu::wgpu::TextureFormat =
    lumen_gpu::wgpu::TextureFormat::Rgba16Float;

/// An sRGB-encoded channel in `0..=1` as linear light.
pub(crate) fn srgb_to_linear(encoded: f32) -> f32 {
    if encoded <= 0.04045 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

/// A straight sRGB colour of 8-bit channels as linear light, premultiplied by its alpha.
pub(crate) fn linear_premultiplied(color: [u8; 4]) -> [f32; 4] {
    let alpha = f32::from(color[3]) / 255.0;
    let [r, g, b] = [color[0], color[1], color[2]]
        .map(|channel| srgb_to_linear(f32::from(channel) / 255.0) * alpha);
    [r, g, b, alpha]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_decode_then_premultiply() {
        assert_eq!(linear_premultiplied([255, 255, 255, 255]), [1.0; 4]);
        assert_eq!(linear_premultiplied([0, 0, 0, 0]), [0.0; 4]);
        let [r, _, _, a] = linear_premultiplied([188, 0, 0, 51]);
        assert!((r - 0.5 * 0.2).abs() < 0.002, "{r}");
        assert_eq!(a, 0.2);
    }
}
