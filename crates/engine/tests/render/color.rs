//! The working space: sources decode to linear light, arithmetic happens there, and the output
//! encodes back to sRGB once. Expected values come from the sRGB curve, worked out here.

use lumen_engine::composition::Composition;
use serde_json::json;

use crate::support::{TestMedia, gpu, render, scene};

/// An sRGB-encoded value in `0..=1` in linear light.
pub fn decode(encoded: f64) -> f64 {
    if encoded <= 0.04045 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear `value`, clamped to `0..=1`, sRGB-encoded.
pub fn encode(value: f64) -> f64 {
    let c = value.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// An sRGB code in linear light.
pub fn linear(code: u8) -> f64 {
    decode(f64::from(code) / 255.0)
}

/// Linear `value` as the nearest sRGB code.
pub fn srgb8(value: f64) -> u8 {
    (encode(value) * 255.0).round() as u8
}

/// A straight sRGB colour of 8-bit channels in linear light, premultiplied.
pub fn premultiplied(color: [u8; 4]) -> [f64; 4] {
    let alpha = f64::from(color[3]) / 255.0;
    [linear(color[0]) * alpha, linear(color[1]) * alpha, linear(color[2]) * alpha, alpha]
}

/// Premultiplied `color` faded by `opacity`.
pub fn scaled(color: [f64; 4], opacity: f64) -> [f64; 4] {
    color.map(|channel| channel * opacity)
}

/// Premultiplied `top` composited over `bottom`.
pub fn over(top: [f64; 4], bottom: [f64; 4]) -> [f64; 4] {
    [0, 1, 2, 3].map(|index| top[index] + bottom[index] * (1.0 - top[3]))
}

/// A premultiplied linear colour as the output encodes it: sRGB-encoded, then premultiplied.
pub fn rgba8(color: [f64; 4]) -> [u8; 4] {
    let alpha = color[3].clamp(0.0, 1.0);
    let channel = |value: f64| {
        let straight = if alpha > 0.0 { value / alpha } else { 0.0 };
        (encode(straight) * alpha * 255.0).round() as u8
    };
    [channel(color[0]), channel(color[1]), channel(color[2]), (alpha * 255.0).round() as u8]
}

/// An opaque pixel of linear `rgb`, as sRGB codes.
pub fn opaque(rgb: [f64; 3]) -> [u8; 4] {
    let [r, g, b] = rgb.map(srgb8);
    [r, g, b, 255]
}

/// Rendered RGBA8 frames, sRGB-encoded with the colour premultiplied, averaged in linear light
/// and encoded as the output encodes it.
pub fn linear_average(frames: &[&[u8]]) -> Vec<u8> {
    let count = frames.len() as f64;
    (0..frames[0].len() / 4)
        .flat_map(|pixel| {
            let mut sum = [0.0; 4];
            for frame in frames {
                let texel = &frame[pixel * 4..][..4];
                let alpha = f64::from(texel[3]) / 255.0;
                for channel in 0..3 {
                    let straight = f64::from(texel[channel]) / 255.0 / alpha.max(1e-9);
                    sum[channel] += decode(straight.min(1.0)) * alpha;
                }
                sum[3] += alpha;
            }
            let alpha = sum[3] / count;
            let channel = |index: usize| {
                let straight = if alpha > 0.0 { sum[index] / count / alpha } else { 0.0 };
                (encode(straight) * alpha * 255.0).round() as u8
            };
            [channel(0), channel(1), channel(2), (alpha * 255.0).round() as u8]
        })
        .collect()
}

fn image_scene(width: u32, height: u32) -> Composition {
    scene(
        (width, height),
        json!([{ "id": 1, "type": "media_in", "params": { "kind": "image", "source": "image" } }]),
    )
    .into()
}

#[test]
fn every_srgb_code_passes_through_unchanged() {
    let rgba: Vec<u8> = (0..=255u8)
        .flat_map(|code| [code, 255 - code, code / 2 + 64, 255])
        .collect();
    let media = TestMedia::default().image("image", 256, 1, rgba.clone());
    let gpu = gpu();
    for (dither, mut renderer) in [(false, gpu.renderer()), (true, gpu.dithered_renderer())] {
        let frame = render(&mut renderer, &image_scene(256, 1), 0.0, &media);
        let changed: Vec<_> = (0..256u32)
            .filter(|&x| frame.pixel(x, 0) != rgba[x as usize * 4..][..4])
            .map(|x| (x, frame.pixel(x, 0)))
            .collect();
        assert!(changed.is_empty(), "dither {dither}: {changed:?}");
    }
}

#[test]
fn half_white_over_black_is_half_the_light() {
    let scene = scene(
        (4, 4),
        json!([
            { "id": 1, "type": "background", "params": { "paint": [0, 0, 0, 255] } },
            { "id": 2, "type": "background", "params": { "paint": [255, 255, 255, 255] } },
            { "id": 3, "type": "merge", "in": { "base": 1, "overlay": 2 }, "params": {
                "opacity": 0.5
            } },
        ]),
    );
    let gpu = gpu();
    let frame = render(&mut gpu.renderer(), &scene.into(), 0.0, &TestMedia::default());
    let expected = srgb8(0.5);
    assert_eq!(expected, 188);
    assert_eq!(frame.pixel(1, 1), [expected, expected, expected, 255]);
}

#[test]
fn json_colours_round_trip_through_linear_light() {
    let gpu = gpu();
    for code in [0, 1, 10, 54, 128, 200, 254, 255] {
        let scene = scene(
            (2, 2),
            json!([{ "id": 1, "type": "background", "params": {
                "paint": [code, 255 - code, code, 255]
            } }]),
        );
        let frame = render(&mut gpu.renderer(), &scene.into(), 0.0, &TestMedia::default());
        assert_eq!(frame.pixel(0, 0), [code, 255 - code, code, 255], "code {code}");
    }
}
