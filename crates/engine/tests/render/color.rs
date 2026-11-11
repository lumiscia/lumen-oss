//! The working space: sources decode to linear light, arithmetic happens there, and the output
//! encodes back to sRGB once. Expected values come from the sRGB curve, worked out here.

use lumen_engine::{composition::Composition, gpu::OutputAlpha};
use serde_json::{Value, json};

use crate::support::{Case, Expect, Frame, Scene, TestMedia, check, gpu, render, scene};

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
    [
        linear(color[0]) * alpha,
        linear(color[1]) * alpha,
        linear(color[2]) * alpha,
        alpha,
    ]
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
    [
        channel(color[0]),
        channel(color[1]),
        channel(color[2]),
        (alpha * 255.0).round() as u8,
    ]
}

/// An opaque pixel of linear `rgb`, as sRGB codes.
pub fn opaque(rgb: [f64; 3]) -> [u8; 4] {
    let [r, g, b] = rgb.map(srgb8);
    [r, g, b, 255]
}

/// An output pixel, sRGB-encoded with its colour premultiplied, as premultiplied linear light.
pub fn light(pixel: [u8; 4]) -> [f64; 4] {
    let alpha = f64::from(pixel[3]) / 255.0;
    let channel = |code: u8| decode((f64::from(code) / 255.0 / alpha.max(1e-9)).min(1.0)) * alpha;
    [
        channel(pixel[0]),
        channel(pixel[1]),
        channel(pixel[2]),
        alpha,
    ]
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
                let straight = if alpha > 0.0 {
                    sum[index] / count / alpha
                } else {
                    0.0
                };
                (encode(straight) * alpha * 255.0).round() as u8
            };
            [
                channel(0),
                channel(1),
                channel(2),
                (alpha * 255.0).round() as u8,
            ]
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
fn every_straight_colour_and_alpha_passes_through_a_straight_output() {
    // Column `x` is code `x`, and row `y` alpha `y + 1`: every 8-bit straight colour at every
    // translucent alpha, as a PNG output takes them.
    let rgba: Vec<u8> = (1..=255u8)
        .flat_map(|alpha| {
            (0..=255u8).flat_map(move |code| [code, 255 - code, code / 2 + 64, alpha])
        })
        .collect();
    let media = TestMedia::default().straight_image("image", 256, 255, rgba.clone());
    let gpu = gpu();
    for (dither, mut renderer) in [(false, gpu.renderer()), (true, gpu.dithered_renderer())] {
        renderer.set_output_alpha(OutputAlpha::Straight);
        let frame = render(&mut renderer, &image_scene(256, 255), 0.0, &media);
        let changed = frame
            .rgba
            .chunks(4)
            .zip(rgba.chunks(4))
            .filter(|(got, want)| got != want)
            .count();
        assert_eq!(
            changed, 0,
            "dither {dither}: {changed} of 65280 pixels changed"
        );
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
    let frame = render(
        &mut gpu.renderer(),
        &scene.into(),
        0.0,
        &TestMedia::default(),
    );
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
        let frame = render(
            &mut gpu.renderer(),
            &scene.into(),
            0.0,
            &TestMedia::default(),
        );
        assert_eq!(
            frame.pixel(0, 0),
            [code, 255 - code, code, 255],
            "code {code}"
        );
    }
}

/// `nodes` as a line, each fed by the one before.
fn chain(canvas: (u32, u32), nodes: Vec<Value>) -> Scene {
    let nodes = nodes.into_iter().enumerate().map(|(index, mut node)| {
        node["id"] = json!(index + 1);
        if index > 0 {
            node["in"] = json!({ "source": index });
        }
        node
    });
    scene(canvas, Value::Array(nodes.collect()))
}

/// A channel of an output pixel as a straight sRGB code.
fn unpremultiplied(pixel: &[u8; 4], channel: usize) -> f64 {
    f64::from(pixel[channel]) * 255.0 / f64::from(pixel[3])
}

fn custom(check: impl Fn(&Frame) + 'static) -> Expect {
    Expect::Custom(Box::new(move |frames| frames.iter().for_each(&check)))
}

#[test]
fn transparent_and_coloured_edges_have_no_dark_fringe() {
    const CLEAR: [u8; 4] = [0, 0, 0, 0];
    // Straight red at alpha 0, uploaded as an image file is: its colour must not show through.
    let hidden_red = TestMedia::default().straight_image("clear", 4, 4, [255, 0, 0, 0].repeat(16));
    let effect = |kind: &str, params: Value| {
        chain(
            (4, 4),
            vec![
                json!({ "type": "media_in", "params": { "kind": "image", "source": "clear" } }),
                json!({ "type": kind, "params": params }),
            ],
        )
    };
    let mut cases: Vec<Case> = [
        ("exposure", json!({ "exposure": 2, "offset": 0.3 })),
        ("levels", json!({ "gamma": 1.5, "output_black": 0.2 })),
        (
            "hue_saturation",
            json!({ "saturation": 1.5, "lightness": 0.2 }),
        ),
        (
            "color_grade",
            json!({ "lut": "rgb1d: 0,0,0; 1,0.5,0", "interpolation": "linear" }),
        ),
        ("blur", json!({ "radius": 2 })),
    ]
    .into_iter()
    .map(|(kind, params)| {
        Case::new(
            format!("{kind} of a transparent straight image"),
            effect(kind, params),
            Expect::Uniform(CLEAR),
        )
        .media(hidden_red.clone())
    })
    .collect();
    let grey = [128, 128, 128, 255];
    cases.push(
        Case::new(
            "multiply of a transparent straight image over grey",
            scene(
                (4, 4),
                json!([
                    { "id": 1, "type": "background", "params": { "paint": grey } },
                    { "id": 2, "type": "media_in", "params": { "kind": "image", "source": "clear" } },
                    { "id": 3, "type": "merge", "in": { "base": 1, "overlay": 2 },
                      "params": { "blend_mode": "multiply" } },
                ]),
            ),
            Expect::Uniform(grey),
        )
        .media(hidden_red),
    );
    // Half of each edge pixel is red over white: red stays full and the rest is half the light.
    let half = srgb8(0.5);
    cases.push(Case::new(
        "a red edge over white",
        scene(
            (16, 4),
            json!([
                { "id": 1, "type": "background", "params": { "paint": [255, 255, 255, 255] } },
                { "id": 2, "type": "shape", "params": {
                    "position": [4.5, 0], "width": 8, "height": 4, "fill_paint": [255, 0, 0, 255]
                } },
                { "id": 3, "type": "merge", "in": { "base": 1, "overlay": 2 } },
            ]),
        ),
        Expect::Probes(vec![
            (3, 1, [255, 255, 255, 255]),
            (4, 1, [255, half, half, 255]),
            (8, 1, [255, 0, 0, 255]),
            (12, 1, [255, half, half, 255]),
            (13, 1, [255, 255, 255, 255]),
        ]),
    ));
    // A colour glyph's fringe is its own colour at partial coverage, so the straight colour of
    // each edge pixel lies within the range of the glyph's opaque colours.
    cases.push(
        Case::new(
            "colour glyph edges keep the glyph's colours",
            scene(
                (64, 64),
                json!([{ "id": 1, "type": "text", "params": {
                    "content": "A", "font_family": "Roboto COLR Test", "font_size": 48,
                    "position": [8.3, 4.6]
                } }]),
            ),
            custom(|frame| {
                let pixels = frame.rgba.as_chunks::<4>().0;
                // The glyph's own colour is slightly translucent, so its interior is the most
                // covered pixels.
                let full = pixels.iter().map(|pixel| pixel[3]).max().unwrap();
                let interior: Vec<_> = pixels.iter().filter(|pixel| pixel[3] == full).collect();
                assert!(interior.len() > 50, "{} interior pixels", interior.len());
                let range = [0, 1, 2].map(|channel| {
                    let values = interior.iter().map(|pixel| unpremultiplied(pixel, channel));
                    (
                        values.clone().fold(f64::INFINITY, f64::min),
                        values.fold(0.0, f64::max),
                    )
                });
                let edges: Vec<_> = pixels
                    .iter()
                    .filter(|pixel| (64..full).contains(&pixel[3]))
                    .collect();
                assert!(edges.len() > 20, "{} edge pixels", edges.len());
                for pixel in edges {
                    let alpha = f64::from(pixel[3]);
                    // A byte of colour is half a code off at most, and unpremultiplying scales that.
                    let slack = 0.5 * 255.0 / alpha + 0.5;
                    for (channel, (low, high)) in range.into_iter().enumerate() {
                        let straight = unpremultiplied(pixel, channel);
                        assert!(
                            straight >= low - slack && straight <= high + slack,
                            "{pixel:?} leaves the glyph's range {range:?}"
                        );
                    }
                }
            }),
        )
        .media(crate::text_pixels::media()),
    );
    check(cases);
}

#[test]
fn a_black_white_edge_filters_in_linear_light() {
    // Black for x < 8 and white from 8, moved half a pixel: pixel 8 is half of each.
    let edge: Vec<u8> = (0..16)
        .flat_map(|x| if x < 8 { [0, 0, 0, 255] } else { [255; 4] })
        .collect();
    let moved = Case::new(
        "half a pixel of translation",
        chain(
            (16, 1),
            vec![
                json!({ "type": "media_in", "params": { "kind": "image", "source": "edge" } }),
                json!({ "type": "transform", "params": {
                    "translate_x": 0.5, "sampling": "linear"
                } }),
            ],
        ),
        Expect::Probes(vec![
            (7, 0, [0, 0, 0, 255]),
            (8, 0, [srgb8(0.5), srgb8(0.5), srgb8(0.5), 255]),
            (9, 0, [255; 4]),
        ]),
    )
    .media(TestMedia::default().image("edge", 16, 1, edge));
    // A symmetric kernel normalised to 1 spreads the edge so that, in linear light, the pixels
    // either side of it sum to 1, whatever the kernel's exact weights.
    let blurred = Case::new(
        "blur",
        scene(
            (64, 64),
            json!([
                { "id": 1, "type": "background", "params": { "paint": [0, 0, 0, 255] } },
                { "id": 2, "type": "shape", "params": {
                    "position": [32, -32], "width": 64, "height": 128,
                    "fill_paint": [255, 255, 255, 255]
                } },
                { "id": 3, "type": "merge", "in": { "base": 1, "overlay": 2 } },
                { "id": 4, "type": "blur", "in": { "source": 3 }, "params": { "radius": 4 } },
            ]),
        ),
        custom(|frame| {
            let first = frame.pixel(31, 32);
            assert!(
                first[0] > 20 && first[0] < 235,
                "the edge is not spread: {first:?}"
            );
            for offset in 0..8 {
                let [left, right] = [31 - offset, 32 + offset].map(|x| frame.pixel(x, 32));
                assert_eq!((left[3], right[3]), (255, 255), "offset {offset}");
                let mirrored = srgb8(1.0 - linear(left[0]));
                assert!(
                    right[0].abs_diff(mirrored) <= 1,
                    "offset {offset}: {left:?} and {right:?}, where {mirrored} mirrors the left"
                );
            }
        }),
    );
    check(vec![moved, blurred]);
}

#[test]
fn a_wide_dark_gradient_keeps_its_precision_until_the_output() {
    // Codes 0 to 8 over 256 pixels, lifted by 6 stops. A gradient rounded to 8 bits before the
    // exposure would show nine steps; in float each pixel is the maths.
    let gradient = |interpolation: &'static str| {
        chain(
            (256, 1),
            vec![
                json!({ "type": "background", "params": {
                    "paint": {
                        "type": "linear_gradient", "start": [0, 0], "end": [1, 0],
                        "interpolation": interpolation,
                        "stops": [[0, [0, 0, 0, 255]], [1, [8, 8, 8, 255]]]
                    },
                    "width": 256, "height": 1, "paint_supersample": false
                } }),
                json!({ "type": "exposure", "params": { "exposure": 6 } }),
            ],
        )
    };
    let expect = |light: fn(f64) -> f64| {
        custom(move |frame| {
            let mut distinct = std::collections::BTreeSet::new();
            for x in 0..256 {
                let along = (f64::from(x) + 0.5) / 256.0;
                let expected = srgb8(64.0 * light(along));
                let pixel = frame.pixel(x, 0);
                assert!(
                    pixel[0].abs_diff(expected) <= 1 && pixel[3] == 255,
                    "x {x}: {pixel:?}, expected {expected}"
                );
                distinct.insert(pixel[0]);
            }
            assert!(distinct.len() > 64, "{} distinct values", distinct.len());
        })
    };
    check(vec![
        Case::new(
            "srgb interpolates the encoded stops",
            gradient("srgb"),
            expect(|along| decode(8.0 / 255.0 * along)),
        ),
        Case::new(
            "linear_srgb interpolates linear light",
            gradient("linear_srgb"),
            expect(|along| linear(8) * along),
        ),
    ]);
}

#[test]
fn blend_modes_work_on_encoded_mid_grey() {
    // On encoded values b = s = 128 / 255: multiply is bs and screen is 1 - (1 - b)(1 - s).
    // In linear light they would be 62 and 166.
    let grey: f64 = 128.0 / 255.0;
    let multiplied = (grey * grey * 255.0).round() as u8;
    let screened = ((1.0 - (1.0 - grey) * (1.0 - grey)) * 255.0).round() as u8;
    assert_eq!((multiplied, screened), (64, 192));
    let blended = |mode: &str| {
        scene(
            (4, 4),
            json!([
                { "id": 1, "type": "background", "params": { "paint": [128, 128, 128, 255] } },
                { "id": 2, "type": "background", "params": { "paint": [128, 128, 128, 255] } },
                { "id": 3, "type": "merge", "in": { "base": 1, "overlay": 2 },
                  "params": { "blend_mode": mode } },
            ]),
        )
    };
    let uniform = |value: u8| Expect::Uniform([value, value, value, 255]);
    check(vec![
        Case::new("multiply", blended("multiply"), uniform(multiplied)),
        Case::new("screen", blended("screen"), uniform(screened)),
    ]);
}

/// A `size` × `size` canvas of the opaque grey whose sRGB code is `code`, which may be fractional,
/// written by a shader so the working space holds it exactly up to half-float rounding.
fn fractional_grey(size: u32, code: f64) -> Composition {
    let shader = format!(
        "
@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {{
    let light = lumen_srgb_to_linear(vec3<f32>({code:.6} / 255.0));
    textureStore(output, vec2<i32>(id.xy), vec4<f32>(light, 1.0));
}}"
    );
    scene(
        (size, size),
        json!([
            { "id": 1, "type": "background", "params": { "paint": [0, 0, 0, 255] } },
            { "id": 2, "type": "wgsl_shader", "in": { "source": [1] }, "params": { "shader": shader } },
        ]),
    )
    .into()
}

#[test]
fn the_output_dither_keeps_the_mean_of_every_fractional_code() {
    // Codes near 5 are in the curve's linear toe, where half floats hold them to 0.007 of a code.
    // An 8 × 8 dither tile rounds the right share of its pixels up, to 1/64 of a code.
    let gpu = gpu();
    for fraction in [0.125, 0.25, 0.4, 0.5, 0.6, 0.75, 0.875] {
        let code = 5.0 + fraction;
        let frame = render(
            &mut gpu.dithered_renderer(),
            &fractional_grey(16, code),
            0.0,
            &TestMedia::default(),
        );
        let reds: Vec<f64> = frame
            .rgba
            .chunks(4)
            .map(|pixel| f64::from(pixel[0]))
            .collect();
        let mean = reds.iter().sum::<f64>() / reds.len() as f64;
        assert!(
            (mean - code).abs() <= 1.0 / 64.0 + 0.01,
            "code {code} averages {mean}"
        );
    }
}

#[test]
fn a_ramp_across_one_code_is_flat_within_the_snap_of_each_end() {
    // 1000 values from code 5 to code 6, each over an 8 × 8 tile so that every dither threshold
    // sees it. A value within a tenth of a code of 5 or 6 is that code exactly, a fifth of the
    // ramp in all; every other tile keeps its value as its mean.
    let shader = "
@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let code = 5.0 + f32(id.x / 8u) / 999.0;
    let light = lumen_srgb_to_linear(vec3<f32>(code / 255.0));
    textureStore(output, vec2<i32>(id.xy), vec4<f32>(light, 1.0));
}";
    let composition: Composition = scene(
        (8000, 8),
        json!([
            { "id": 1, "type": "background", "params": { "paint": [0, 0, 0, 255] } },
            { "id": 2, "type": "wgsl_shader", "in": { "source": [1] }, "params": { "shader": shader } },
        ]),
    )
    .into();
    let gpu = gpu();
    let frame = render(
        &mut gpu.dithered_renderer(),
        &composition,
        0.0,
        &TestMedia::default(),
    );
    let mut flat = 0;
    for tile in 0..1000u32 {
        let reds: Vec<u8> = (0..8)
            .flat_map(|y| (0..8).map(move |x| (tile * 8 + x, y)))
            .map(|(x, y)| frame.pixel(x, y)[0])
            .collect();
        let fraction = f64::from(tile) / 999.0;
        let mean = reds.iter().map(|&red| f64::from(red)).sum::<f64>() / 64.0;
        let uniform = reds.iter().all(|&red| red == reds[0]);
        flat += usize::from(uniform);
        if fraction < 0.09 || fraction > 0.91 {
            assert!(
                uniform && f64::from(reds[0]) == 5.0 + fraction.round(),
                "tile {tile}: {reds:?}"
            );
        } else if (0.11..0.89).contains(&fraction) {
            assert!(!uniform, "tile {tile} is flat");
            assert!(
                (mean - 5.0 - fraction).abs() <= 1.0 / 64.0 + 0.01,
                "tile {tile}: {mean}"
            );
        }
    }
    assert!((190..=210).contains(&flat), "{flat} of 1000 tiles are flat");
}

#[test]
fn quantised_colour_never_exceeds_its_quantised_alpha() {
    let gpu = gpu();
    for opacity in [0.0019, 0.002, 0.003, 0.0045, 0.01, 0.3] {
        let composition = scene(
            (16, 16),
            json!([
                { "id": 1, "type": "background", "params": { "paint": [255, 255, 255, 255] } },
                { "id": 2, "type": "opacity", "in": { "source": 1 }, "params": { "opacity": opacity } },
            ]),
        )
        .into();
        let frame = render(
            &mut gpu.dithered_renderer(),
            &composition,
            0.0,
            &TestMedia::default(),
        );
        let bad: Vec<_> = frame
            .rgba
            .chunks(4)
            .filter(|pixel| pixel[..3].iter().any(|&c| c > pixel[3]))
            .collect();
        assert!(
            bad.is_empty(),
            "opacity {opacity}: {} pixels like {:?}",
            bad.len(),
            bad[0]
        );
    }
}

#[test]
fn a_float_output_is_the_working_space_without_an_output_pass() {
    // A Vulkan Video or NVENC target converts the working space itself, so `media_output` hands
    // it the merged frame rather than writing a copy.
    let gpu = gpu();
    let scene: Composition = scene(
        (16, 16),
        json!([
            { "id": 1, "type": "background", "params": { "paint": [255, 0, 0, 255] } },
            { "id": 2, "type": "background", "params": { "paint": [0, 0, 255, 128] } },
            { "id": 3, "type": "merge", "in": { "base": 1, "overlay": 2 } }
        ]),
    )
    .into();
    let passes = |format| {
        let mut renderer = gpu.renderer();
        renderer.set_static_output_reuse(false);
        renderer.compile_with_output_format(&scene, format).unwrap();
        renderer
            .render_frame_at(&scene, 0.0, &TestMedia::default())
            .unwrap();
        renderer.submitted_pass_count()
    };
    let float = passes(lumen_gpu::wgpu::TextureFormat::Rgba16Float);
    let bytes = passes(lumen_gpu::wgpu::TextureFormat::Rgba8Unorm);
    assert_eq!(
        float + 1,
        bytes,
        "{float} passes to a float output, {bytes} to 8-bit"
    );
}

#[test]
fn extreme_parameters_store_finite_light() {
    // Every case checks the working space holds finite values, so these rows are the invariant at
    // each node whose arithmetic can overflow a half float or make NaN from finite parameters.
    let grey =
        json!({ "id": 1, "type": "background", "params": { "paint": [128, 128, 128, 255] } });
    let exposure = |id: u32, from: u32, params: Value| json!({ "id": id, "type": "exposure", "params": params, "in": { "source": from } });
    // Translucent light far brighter than its alpha.
    let bright = || {
        json!([
            { "id": 1, "type": "background", "params": { "paint": [255, 255, 255, 128] } },
            exposure(2, 1, json!({ "exposure": 17 })),
        ])
    };
    let after_bright = |node: Value| {
        let Value::Array(mut nodes) = bright() else {
            unreachable!()
        };
        nodes.push(node);
        Value::Array(nodes)
    };
    let rows: Vec<(&str, Value)> = vec![
        (
            "exposure",
            json!([grey, exposure(2, 1, json!({ "exposure": 129 }))]),
        ),
        (
            "exposure, then opacity 0",
            json!([
                grey,
                exposure(2, 1, json!({ "exposure": 129 })),
                { "id": 3, "type": "opacity", "params": { "opacity": 0 }, "in": { "source": 2 } }
            ]),
        ),
        (
            "exposure contrast and offset",
            json!([
                grey,
                exposure(2, 1, json!({ "contrast": 1e30, "offset": 1e30 }))
            ]),
        ),
        (
            "levels",
            json!([grey, { "id": 2, "type": "levels", "in": { "source": 1 },
                "params": { "gamma": 1e30, "black_point": 0.5, "white_point": 0.5 } }]),
        ),
        (
            "levels past the f32 range",
            json!([grey, { "id": 2, "type": "levels", "in": { "source": 1 },
                "params": { "gamma": 1e39, "black_point": -1e39, "white_point": 1e39 } }]),
        ),
        (
            "saturation past the f32 range, of grey",
            json!([grey, { "id": 2, "type": "hue_saturation", "in": { "source": 1 },
                "params": { "saturation": 1e39, "lightness": -1e39 } }]),
        ),
        (
            "hue and saturation",
            after_bright(
                json!({ "id": 3, "type": "hue_saturation", "in": { "source": 2 },
                "params": { "hue_degrees": 1e30, "saturation": 1e30, "lightness": 1e30 } }),
            ),
        ),
        (
            "normal merge",
            json!([
                grey,
                exposure(2, 1, json!({ "exposure": 40 })),
                { "id": 3, "type": "background", "params": { "paint": [255, 255, 255, 128] } },
                exposure(4, 3, json!({ "exposure": 17 })),
                { "id": 5, "type": "merge", "in": { "base": 2, "overlay": 4 } }
            ]),
        ),
        (
            "glow",
            after_bright(json!({ "id": 3, "type": "glow", "in": { "source": 2 },
                "params": { "threshold": 0, "radius": 0, "intensity": 1000 } })),
        ),
        (
            "shadow",
            after_bright(json!({ "id": 3, "type": "shadow", "in": { "source": 2 },
                "params": { "radius": 0, "opacity": 1 } })),
        ),
    ];
    check(
        rows.into_iter()
            .map(|(name, nodes)| {
                Case::new(name, scene((8, 8), nodes), Expect::Custom(Box::new(|_| {})))
                    .check_working_space()
            })
            .collect(),
    );
}

#[cfg(feature = "3d")]
#[test]
fn emissive_light_past_a_half_float_stays_finite() {
    check(
        ["none", "aces", "agx"]
            .into_iter()
            .map(|tone_mapping| {
                Case::new(
                    format!("3D emissive light, {tone_mapping}"),
                    crate::scene::render_3d_with(
                        json!({ "tone_mapping": tone_mapping }),
                        vec![
                            json!({ "id": 1, "type": "shape_3d", "params": { "emissive": 1e30 } }),
                        ],
                    ),
                    Expect::Custom(Box::new(|_| {})),
                )
                .check_working_space()
            })
            .collect(),
    );
}

/// A `.cube` file of `size` points per axis whose entry for lattice point `(r, g, b)` is
/// `entry(r, g, b)`, red varying fastest.
fn cube_file(size: u32, entry: impl Fn(u32, u32, u32) -> [f64; 3]) -> String {
    let mut text = format!("TITLE \"test\"\nLUT_3D_SIZE {size}\n");
    for b in 0..size {
        for g in 0..size {
            for r in 0..size {
                let [x, y, z] = entry(r, g, b);
                text += &format!("{x} {y} {z}\n");
            }
        }
    }
    text
}

/// `pixels` through `color_grade` with the `.cube` file `text`.
fn grade_with_cube(text: &str, params: Value, pixels: &[[u8; 4]], straight: bool) -> Vec<[u8; 4]> {
    let rgba: Vec<u8> = pixels.iter().flatten().copied().collect();
    let width = pixels.len() as u32;
    let media = if straight {
        TestMedia::default().straight_image("image", width, 1, rgba)
    } else {
        TestMedia::default().image("image", width, 1, rgba)
    };
    let media = media.lut("grade.cube", text);
    let mut params = params;
    params["lut_source"] = json!("grade.cube");
    let composition: Composition = chain(
        (width, 1),
        vec![
            json!({ "type": "media_in", "params": { "kind": "image", "source": "image" } }),
            json!({ "type": "color_grade", "params": params }),
        ],
    )
    .into();
    let mut renderer = gpu().renderer();
    let first = render(&mut renderer, &composition, 0.0, &media);
    render(&mut renderer, &composition, 0.0, &media);
    assert_eq!(media.lut_loads(), 1, "the table is read once per identity");
    (0..width).map(|x| first.pixel(x, 0)).collect()
}

#[test]
fn cube_luts_sample_the_lattice_and_interpolate_in_the_encoded_domain() {
    let linear_cube = |size: u32| {
        let last = f64::from(size - 1);
        cube_file(size, |r, g, b| {
            [
                f64::from(r) / last,
                f64::from(g) / last,
                f64::from(b) / last,
            ]
        })
    };

    // An identity table of an odd size leaves every code within one.
    let ramp: Vec<[u8; 4]> = (0..=255u8)
        .map(|code| [code, 255 - code, code / 2 + 64, 255])
        .collect();
    let identity = grade_with_cube(&linear_cube(17), json!({}), &ramp, false);
    for (code, (out, source)) in identity.iter().zip(&ramp).enumerate() {
        for channel in 0..4 {
            assert!(
                out[channel].abs_diff(source[channel]) <= 1,
                "code {code}: {out:?} != {source:?}"
            );
        }
    }

    // Swapping red and blue does exactly that at the lattice points.
    let corners: Vec<[u8; 4]> = (0..8u8)
        .map(|n| [(n & 1) * 255, (n >> 1 & 1) * 255, (n >> 2 & 1) * 255, 255])
        .collect();
    let swapped = grade_with_cube(
        &cube_file(2, |r, g, b| [b, g, r].map(f64::from)),
        json!({}),
        &corners,
        false,
    );
    for (out, source) in swapped.iter().zip(&corners) {
        assert_eq!(*out, [source[2], source[1], source[0], 255]);
    }

    // A size-2 table is trilinear between its corners, which a calculation in the encoded domain
    // repeats: the lookup takes the encoded colour and the output decodes the table's value.
    let corner = |r: u32, g: u32, b: u32| {
        [
            0.1 + 0.8 * f64::from(r) * f64::from(g),
            0.9 - 0.7 * f64::from(g) + 0.3 * f64::from(b),
            0.2 + 0.5 * f64::from(r) + 0.2 * f64::from(b) * f64::from(g),
        ]
    };
    let input = [128u8, 64, 200, 255];
    let weights = [128.0 / 255.0, 64.0 / 255.0, 200.0 / 255.0];
    let mut expected = [0.0; 3];
    for (corner_index, bits) in (0..8u32).enumerate() {
        let (r, g, b) = (bits & 1, bits >> 1 & 1, bits >> 2 & 1);
        let weight: f64 = [r, g, b]
            .iter()
            .zip(weights)
            .map(|(&bit, w)| if bit == 1 { w } else { 1.0 - w })
            .product();
        let value = corner(r, g, b);
        for channel in 0..3 {
            expected[channel] += weight * value[channel];
        }
        let _ = corner_index;
    }
    let out = grade_with_cube(&cube_file(2, corner), json!({}), &[input], false)[0];
    for channel in 0..3 {
        let code = (expected[channel] * 255.0).round();
        assert!(
            (f64::from(out[channel]) - code).abs() <= 2.0,
            "channel {channel}: {out:?}, expected {code}"
        );
    }

    // Strength blends the graded colour with the source, as it does for the JSON table.
    let half = grade_with_cube(
        &cube_file(2, |r, g, b| [b, g, r].map(f64::from)),
        json!({ "strength": 0.5 }),
        &[[255, 0, 0, 255]],
        false,
    )[0];
    assert!(
        half[0].abs_diff(128) <= 1 && half[2].abs_diff(128) <= 1 && half[1] == 0,
        "{half:?}"
    );

    // A translucent pixel keeps its alpha, and its colour is graded straight.
    let faded = grade_with_cube(
        &cube_file(2, |r, g, b| [b, g, r].map(f64::from)),
        json!({}),
        &[[255, 0, 0, 128]],
        true,
    )[0];
    assert_eq!(faded[3], 128);
    assert!(
        faded[0] == 0 && faded[1] == 0 && (unpremultiplied(&faded, 2) - 255.0).abs() < 2.0,
        "{faded:?}"
    );
}
