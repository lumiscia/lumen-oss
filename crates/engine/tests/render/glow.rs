//! Glow against its definition: the light of a uniform source is worked out by hand, a white
//! source at threshold 0 glows exactly as it blurs, and sources sized to their bounds draw what
//! full-frame sources draw.

use serde_json::{Value, json};

use crate::color::{decode, encode, linear};
use crate::support::{Case, Expect, Frame, Scene, TestMedia, check, gpu, render, scene};

const CANVAS: (u32, u32) = (96, 64);
const LUMA: [f64; 3] = [0.2126, 0.7152, 0.0722];

fn glow(params: Value) -> Value {
    json!({ "id": 2, "type": "glow", "params": params, "in": { "source": 1 } })
}

fn glowing(canvas: (u32, u32), source: Value, params: Value) -> Scene {
    scene(canvas, json!([source, glow(params)]))
}

fn fill(color: [u8; 4]) -> Value {
    json!({ "id": 1, "type": "background", "params": { "paint": color } })
}

fn square(position: Value, size: u32, color: [u8; 4]) -> Value {
    json!({ "id": 1, "type": "shape", "params": {
        "position": position, "width": size, "height": size, "fill_paint": color } })
}

/// The light an opaque `color` casts at `threshold`, in linear light, before it is blurred: its
/// luminance, sRGB-encoded, is remapped from the threshold up, and the colour scaled to it.
fn light(color: [u8; 3], threshold: f64) -> [f64; 3] {
    let rgb = color.map(linear);
    let luma: f64 = rgb.iter().zip(LUMA).map(|(c, w)| c * w).sum();
    let ramp = ((encode(luma) - threshold) / (1.0 - threshold).max(1.0 / 255.0)).clamp(0.0, 1.0);
    rgb.map(|channel| channel * ramp / luma)
}

/// Premultiplied linear `rgb` with `alpha`, as the output encodes it.
fn bytes(rgb: [f64; 3], alpha: f64) -> [u8; 4] {
    let alpha = alpha.clamp(0.0, 1.0);
    let channel = |value: f64| (encode(value / alpha) * alpha * 255.0).round() as u8;
    [
        channel(rgb[0]),
        channel(rgb[1]),
        channel(rgb[2]),
        (alpha * 255.0).round() as u8,
    ]
}

/// A uniform source blurs to itself, so its glow is its light everywhere.
#[test]
fn glow_of_a_uniform_source() {
    let color = [200, 100, 50];
    let opaque = [200, 100, 50, 255];
    let lit = light(color, 0.2);
    let peak = lit.into_iter().fold(0.0, f64::max);
    let added = [0, 1, 2].map(|i| linear(color[i]) + lit[i] * 0.5);
    // White's light is white, of luminance 1, so it takes the premultiplied tint.
    let alpha = 128.0 / 255.0;
    let tinted = [0.0, decode(128.0 / 255.0) * alpha, alpha];
    check(vec![
        Case::new(
            "glow only is the light, with its brightest channel as alpha",
            glowing(
                CANVAS,
                fill(opaque),
                json!({ "threshold": 0.2, "radius": 9, "glow_only": true }),
            ),
            Expect::Uniform(bytes(lit, peak)),
        ),
        Case::new(
            "added to the source at half intensity",
            glowing(
                CANVAS,
                fill(opaque),
                json!({ "threshold": 0.2, "radius": 30, "intensity": 0.5 }),
            ),
            Expect::Uniform(bytes(added, 1.0)),
        ),
        Case::new(
            "a half-transparent tint takes the light's luminance",
            glowing(
                CANVAS,
                fill([255; 4]),
                json!({ "threshold": 0, "radius": 4, "glow_only": true, "tint_enabled": true,
                    "tint": [0, 128, 255, 128] }),
            ),
            Expect::Uniform(bytes(tinted, tinted[2])),
        ),
        Case::new(
            "nothing at or below the threshold glows",
            glowing(
                CANVAS,
                fill([100, 100, 100, 255]),
                json!({ "threshold": 0.4, "radius": 6, "intensity": 3 }),
            ),
            Expect::Uniform([100, 100, 100, 255]),
        ),
        Case::new(
            "black casts no light",
            glowing(
                CANVAS,
                fill([0, 0, 0, 255]),
                json!({ "threshold": 0, "radius": 6, "glow_only": true }),
            ),
            Expect::Uniform([0; 4]),
        ),
        Case::new(
            "a threshold of 1 lets nothing glow",
            glowing(
                CANVAS,
                fill([255; 4]),
                json!({ "threshold": 1, "radius": 6, "glow_only": true }),
            ),
            Expect::Uniform([0; 4]),
        ),
    ]);
}

/// A white canvas glows white to its edges where they clamp, and fades out towards them where
/// they read as transparent.
#[test]
fn glow_edges_clamp_or_fade() {
    let at = |edge: &str| {
        glowing(
            CANVAS,
            fill([255; 4]),
            json!({ "threshold": 0, "radius": 8, "glow_only": true, "edge": edge }),
        )
    };
    check(vec![
        Case::new("clamped edges", at("clamp"), Expect::Uniform([255; 4])),
        Case::new(
            "transparent edges",
            at("transparent"),
            Expect::Custom(Box::new(|frames| {
                let frame = &frames[0];
                let (centre, side, corner) =
                    (frame.pixel(48, 32), frame.pixel(0, 32), frame.pixel(0, 0));
                assert_eq!(centre, [255; 4]);
                // Half of a Gaussian centred on the edge is on the canvas, and a quarter at a
                // corner, with a little more for the texel's own width.
                assert!((128..170).contains(&side[3]), "side {side:?}");
                assert!((64..side[3]).contains(&corner[3]), "corner {corner:?}");
            })),
        ),
    ]);
}

fn render_scene(scene: Scene) -> Frame {
    let gpu = gpu();
    render(
        &mut gpu.renderer(),
        &scene.into(),
        0.0,
        &TestMedia::default(),
    )
}

/// White is its own light at threshold 0, so its glow is its blur, which is added to it.
#[test]
fn white_glows_as_it_blurs() {
    for radius in [3.0, 25.0, 120.0] {
        let source = || square(json!([30, 20]), 24, [255; 4]);
        let blurred = render_scene(scene(
            CANVAS,
            json!([source(), { "id": 2, "type": "blur", "params": { "radius": radius },
                "in": { "source": 1 } }]),
        ));
        let plain = render_scene(scene(CANVAS, json!([source()])));
        for (glow_only, expected) in [
            (true, blurred.rgba.clone()),
            (
                false,
                (blurred.rgba.iter().zip(&plain.rgba))
                    .map(|(a, b)| a.saturating_add(*b))
                    .collect(),
            ),
        ] {
            let glowed = render_scene(glowing(
                CANVAS,
                source(),
                json!({ "threshold": 0, "radius": radius, "glow_only": glow_only }),
            ));
            assert!(blurred.rgba.iter().any(|&byte| byte > 0));
            if let Err(error) = crate::support::within_rounding(&glowed.rgba, &expected) {
                panic!("radius {radius}, glow only {glow_only}: {error}");
            }
        }
    }
}

#[test]
fn glow_is_deterministic_and_continuous_in_threshold() {
    let at = |threshold: f64| {
        render_scene(glowing(
            CANVAS,
            square(json!([20.5, 14.25]), 30, [255, 180, 40, 230]),
            json!({ "threshold": threshold, "radius": 10, "intensity": 2 }),
        ))
    };
    let base = at(0.5);
    assert_eq!(base.rgba, at(0.5).rgba);
    let stepped = at(0.505);
    assert_ne!(base.rgba, stepped.rgba);
    let jump = (base.rgba.iter().zip(&stepped.rgba))
        .map(|(a, b)| a.abs_diff(*b))
        .max();
    assert!(jump <= Some(3), "a small threshold step jumps {jump:?}");
}

/// Intensity is capped where a single white pixel's light still fits the bright pass's half
/// floats, so the blur spreads all of it.
#[test]
fn intensity_past_the_cap_glows_as_the_cap() {
    let at = |intensity: f64| {
        render_scene(glowing(
            (32, 32),
            square(json!([8, 8]), 1, [255; 4]),
            json!({ "threshold": 0, "radius": 4, "intensity": intensity, "glow_only": true }),
        ))
    };
    let capped = at(1000.0);
    assert!(capped.rgba.iter().any(|&byte| byte > 0 && byte < 255));
    assert_eq!(capped.rgba, at(1e300).rgba);
}

/// Sources sized to their bounds, grown by the blur's reach, draw what full-frame sources draw.
#[test]
fn glow_bounds_match_reference() {
    let canvas = (384, 288);
    let small = |position: Value| square(position, 14, [255, 230, 120, 210]);
    let mut cases = Vec::new();
    for radius in [0.0, 6.0, 21.0, 40.0, 100.0] {
        for position in [json!([2, 3]), json!([368, 274]), json!([-5, 270])] {
            for glow_only in [false, true] {
                cases.push(
                    Case::new(
                        format!("radius {radius} at {position}, glow only {glow_only}"),
                        glowing(
                            canvas,
                            small(position.clone()),
                            json!({ "threshold": 0.3, "radius": radius, "intensity": 1.5,
                                "glow_only": glow_only }),
                        ),
                        Expect::Nonblank,
                    )
                    .bounded(true),
                );
            }
        }
    }
    cases.push(
        Case::new(
            "animated and tinted",
            glowing(
                canvas,
                small(json!("=vec2(40 + frame * 30, 60)")),
                json!({ "threshold": "=0.2 + frame / 20", "radius": "=4 + frame * 6",
                    "tint_enabled": true, "tint": [80, 160, 255, 255] }),
            )
            .frames(6),
            Expect::Nonblank,
        )
        .frames([0.0, 2.0, 5.0])
        .bounded(true),
    );
    cases.push(
        Case::new(
            "a full-frame source",
            glowing(
                canvas,
                json!({ "id": 1, "type": "fractal_noise", "params": { "scale": 30, "contrast": 2 } }),
                json!({ "threshold": 0.5, "radius": 15 }),
            ),
            Expect::Nonblank,
        )
        .bounded(false),
    );
    check(cases);
}

#[test]
fn glow_only_light_keeps_its_headroom_with_alpha_at_most_one() {
    // White at threshold 0 and radius 0 is `intensity` times white. Its alpha is 1, so taking
    // the stops back off with `exposure` gives white again.
    let cases = [2.0f64, 10.0]
        .map(|intensity| {
            Case::new(
                format!("intensity {intensity}"),
                scene(
                    (8, 8),
                    json!([
                        fill([255, 255, 255, 255]),
                        glow(json!({
                            "threshold": 0, "radius": 0, "intensity": intensity, "glow_only": true
                        })),
                        { "id": 3, "type": "exposure", "in": { "source": 2 },
                          "params": { "exposure": -intensity.log2() } },
                    ]),
                ),
                Expect::Uniform([255, 255, 255, 255]),
            )
        })
        .into();
    check(cases);
}
