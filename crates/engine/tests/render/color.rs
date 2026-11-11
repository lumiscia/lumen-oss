//! The working space: sources decode to linear light, arithmetic happens there, and the output
//! encodes back to sRGB once. Expected values come from the sRGB curve, worked out here.

use lumen_engine::composition::Composition;
use serde_json::json;

use crate::support::{TestMedia, gpu, render, scene};

/// An sRGB code in linear light.
pub fn linear(code: u8) -> f64 {
    let c = f64::from(code) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear `value` as the nearest sRGB code.
pub fn srgb8(value: f64) -> u8 {
    let c = value.clamp(0.0, 1.0);
    let encoded = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round() as u8
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
