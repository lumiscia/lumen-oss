//! The working colour space and the sRGB transfer function on the CPU, matching `color.wgsl`.

/// The format of every raster the compositor writes: linear light with BT.709 primaries and
/// premultiplied alpha, where 1.0 is SDR white and colour may go past it. Every node keeps alpha
/// within `0..=1`, while colour may be brighter than its alpha, as light such as glow is.
pub const WORKING_COLOR_FORMAT: lumen_gpu::wgpu::TextureFormat =
    lumen_gpu::wgpu::TextureFormat::Rgba16Float;

/// An sRGB-encoded channel as linear light. Values outside `0..=1` follow the curve mirrored
/// about zero, as `lumen_srgb_to_linear` does.
pub(crate) fn srgb_to_linear(encoded: f32) -> f32 {
    let c = encoded.abs();
    let linear = if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    };
    linear.copysign(encoded)
}

/// Linear light within `0..=1` as an sRGB-encoded channel.
pub(crate) fn linear_to_srgb(linear: f32) -> f32 {
    if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
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

    #[test]
    fn the_curve_is_mirrored_about_zero_and_keeps_white() {
        assert_eq!(srgb_to_linear(1.0), 1.0);
        assert_eq!(srgb_to_linear(0.0), 0.0);
        for encoded in [0.01, 0.04045, 0.2, 0.5, 1.0, 1.5, 2.0] {
            assert_eq!(
                srgb_to_linear(-encoded),
                -srgb_to_linear(encoded),
                "{encoded}"
            );
        }
        assert!((srgb_to_linear(2.0) - 4.9538).abs() < 1e-3);
    }

    /// `value` as a half float, rounded to nearest or, as AMD stores them, toward zero.
    fn half(value: f32, toward_zero: bool) -> f32 {
        let exponent = ((value.to_bits() >> 23) & 0xff) as i32 - 127;
        let step = 2f32.powi(exponent.max(-14) - 10);
        let steps = value / step;
        if toward_zero {
            steps.trunc() * step
        } else {
            steps.round_ties_even() * step
        }
    }

    fn encode(linear: f32) -> f32 {
        if linear <= 0.003_130_8 {
            linear * 12.92
        } else {
            let p = linear.powf(1.0 / 2.4);
            p + 0.055 * (p - 1.0)
        }
    }

    /// `LUMEN_SNAP` from the dither chunk.
    fn snap() -> f32 {
        let source = include_str!("shaders/dither.wgsl");
        let line = source
            .lines()
            .find_map(|line| line.strip_prefix("const LUMEN_SNAP: f32 = "))
            .expect("dither.wgsl defines LUMEN_SNAP");
        line.trim_end_matches(';').parse().expect("a number")
    }

    #[test]
    fn half_floats_keep_every_8_bit_code_inside_the_snap() {
        // Each 8-bit straight colour and alpha as the working space stores them, and as the
        // output reads them back: opaque, translucent and straight, and NV12 luma. The error must
        // stay inside `LUMEN_SNAP` for 8-bit content to come out exactly.
        let snap = snap();
        for toward_zero in [false, true] {
            let mut worst = [0f32; 3];
            for code in 0..=255u8 {
                let light = srgb_to_linear(f32::from(code) / 255.0);
                let stored = half(light, toward_zero);
                worst[0] = worst[0].max((encode(stored) * 255.0 - f32::from(code)).abs());
                worst[2] = worst[2].max((encode(stored) - f32::from(code) / 255.0).abs() * 219.0);
                for alpha in 1..255u8 {
                    let a = f32::from(alpha) / 255.0;
                    let straight = half(light * a, toward_zero) / half(a, toward_zero);
                    worst[1] = worst[1].max((encode(straight) * 255.0 - f32::from(code)).abs());
                }
            }
            for (path, error) in ["opaque", "translucent", "NV12 luma"].iter().zip(worst) {
                assert!(
                    error < snap,
                    "{path}, toward zero {toward_zero}: {error} codes, snap {snap}"
                );
            }
        }
    }
}
