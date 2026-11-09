//! Directional, radial and zoom blurs. A line blur whose taps land on whole pixels is a box blur
//! worked out by hand; spins and zooms keep what their moves keep; and sources sized to their
//! bounds draw what full-frame sources draw.

use serde_json::{Value, json};

use crate::support::{Case, Expect, Frame, Scene, TestMedia, check, gpu, render, scene};

fn blurred(canvas: (u32, u32), source: Value, kind: &str, params: Value) -> Scene {
    scene(
        canvas,
        json!([source, { "id": 2, "type": kind, "params": params, "in": { "source": 1 } }]),
    )
}

fn rect(position: [f64; 2], size: [f64; 2], color: [u8; 4]) -> Value {
    json!({ "id": 1, "type": "shape", "params": {
        "position": position, "width": size[0], "height": size[1], "fill_paint": color } })
}

fn fill(color: [u8; 4]) -> Value {
    json!({ "id": 1, "type": "background", "params": { "paint": color } })
}

/// A box of `taps` whole pixels over `covered`, the pixels of the source a pixel's taps land on.
fn box_alpha(covered: usize, taps: usize) -> u8 {
    (255.0 * covered as f64 / taps as f64).round() as u8
}

/// The line blur of a white bar over pixels 8 to 11 across the blur, at each of the first 24
/// pixels along it.
fn bar_profile(taps: i32) -> Vec<u8> {
    let half = taps / 2;
    (0..24)
        .map(|x: i32| {
            let covered = (-half..=half)
                .filter(|k| (8..12).contains(&(x + k)))
                .count();
            box_alpha(covered, taps as usize)
        })
        .collect()
}

#[test]
fn line_blurs_are_box_blurs() {
    let row = |profile: Vec<u8>, vertical: bool| {
        let probes = profile.into_iter().enumerate().map(|(i, a)| {
            let (x, y) = if vertical {
                (3, i as u32)
            } else {
                (i as u32, 3)
            };
            (x, y, [a; 4])
        });
        Expect::Probes(probes.collect())
    };
    let across = |angle: f64, distance: f64| {
        blurred(
            (32, 8),
            rect([8.0, 0.0], [4.0, 8.0], [255; 4]),
            "directional_blur",
            json!({ "angle": angle, "distance": distance }),
        )
    };
    let red = |edge: &str| {
        blurred(
            (24, 6),
            fill([255, 0, 0, 255]),
            "directional_blur",
            json!({ "distance": 9, "edge": edge }),
        )
    };
    // Nine taps from 4 pixels left to 4 right: the canvas holds 5 of them at its edge.
    let red_at = |x: u32, covered: usize| {
        let a = box_alpha(covered, 9);
        (x, 2, [a, 0, 0, a])
    };
    check(vec![
        Case::new(
            "9 pixels across",
            across(0.0, 9.0),
            row(bar_profile(9), false),
        ),
        Case::new(
            "25 pixels across",
            across(0.0, 25.0),
            row(bar_profile(25), false),
        ),
        Case::new(
            "backwards is the same",
            across(180.0, 9.0),
            row(bar_profile(9), false),
        ),
        Case::new(
            "9 pixels down",
            blurred(
                (8, 32),
                rect([0.0, 8.0], [8.0, 4.0], [255; 4]),
                "directional_blur",
                json!({ "angle": 90, "distance": 9 }),
            ),
            row(bar_profile(9), true),
        ),
        Case::new(
            "clamped edges repeat",
            red("clamp"),
            Expect::Uniform([255, 0, 0, 255]),
        ),
        Case::new(
            "transparent edges fade",
            red("transparent"),
            Expect::Probes(vec![
                red_at(0, 5),
                red_at(1, 6),
                red_at(3, 8),
                red_at(4, 9),
                red_at(12, 9),
                red_at(23, 5),
            ]),
        ),
    ]);
}

#[test]
fn spins_and_zooms_keep_what_their_moves_keep() {
    let canvas = (64, 64);
    let uniform = |kind: &str, amount: f64| {
        Case::new(
            format!("{kind} {amount} of a uniform fill"),
            blurred(
                canvas,
                fill([30, 90, 200, 255]),
                kind,
                json!({ "center": [20, 41], "amount": amount }),
            ),
            Expect::Uniform([30, 90, 200, 255]),
        )
    };
    // A square on the centre: the centre pixel only ever reads itself.
    let centred = |kind: &str, amount: f64| {
        Case::new(
            format!("{kind} {amount} keeps the centre"),
            blurred(
                canvas,
                rect([28.0, 28.0], [8.0, 8.0], [255, 200, 0, 255]),
                kind,
                json!({ "center": [32.5, 32.5], "amount": amount, "edge": "transparent" }),
            ),
            Expect::Probes(vec![(32, 32, [255, 200, 0, 255])]),
        )
    };
    let full_turn = Case::new(
        "a full turn is the same at every angle",
        blurred(
            canvas,
            rect([44.0, 30.0], [6.0, 4.0], [255; 4]),
            "radial_blur",
            json!({ "center": [32.5, 32.5], "amount": 360, "edge": "transparent" }),
        ),
        Expect::Custom(Box::new(|frames| {
            let frame = &frames[0];
            // The square spans radii 12 to 18, and pixel 32 is on the centre.
            for radius in [14, 16] {
                let around = [
                    frame.pixel(32 + radius, 32),
                    frame.pixel(32, 32 + radius),
                    frame.pixel(32 - radius, 32),
                    frame.pixel(32, 32 - radius),
                ];
                assert!(around[0][3] > 5, "radius {radius} is blank: {around:?}");
                for pixel in around {
                    assert!(
                        pixel.iter().zip(around[0]).all(|(a, b)| a.abs_diff(b) <= 2),
                        "radius {radius}: {around:?}"
                    );
                }
            }
        })),
    );
    // A bar on the row through the centre streaks along the row and nowhere else. Scales run from
    // 0.74 to 1.34, so the pixels from radius 22 to 48 read the bar at radii 30 to 36.
    let ray = Case::new(
        "a zoom streaks along its rays",
        blurred(
            canvas,
            rect([40.0, 30.0], [6.0, 4.0], [255; 4]),
            "zoom_blur",
            json!({ "center": [10, 32], "amount": 0.6, "edge": "transparent" }),
        ),
        Expect::Custom(Box::new(|frames| {
            let frame = &frames[0];
            for x in [34, 38, 46, 50] {
                assert!(frame.pixel(x, 31)[3] > 20, "({x}, 31) has no streak");
            }
            for (x, y) in [(43, 24), (43, 39), (25, 31), (62, 31)] {
                assert_eq!(frame.pixel(x, y), [0; 4], "({x}, {y}) is off the rays");
            }
        })),
    );
    check(vec![
        uniform("radial_blur", 40.0),
        uniform("zoom_blur", 0.5),
        centred("radial_blur", 90.0),
        centred("zoom_blur", 0.8),
        full_turn,
        ray,
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

/// The same params render the same pixels, and a step that changes the tap count moves the
/// output only as far as the step does.
#[test]
fn sweeps_are_deterministic_and_smooth_across_tap_counts() {
    let source = || rect([30.5, 20.25], [18.0, 13.0], [255, 120, 40, 230]);
    for (kind, params, stepped) in [
        (
            "directional_blur",
            json!({ "angle": 30, "distance": 16 }),
            json!({ "angle": 30, "distance": 16.25 }),
        ),
        (
            "radial_blur",
            json!({ "center": [20, 30], "amount": 18 }),
            json!({ "center": [20, 30], "amount": 18.2 }),
        ),
        (
            "zoom_blur",
            json!({ "center": [20, 30], "amount": 0.25 }),
            json!({ "center": [20, 30], "amount": 0.252 }),
        ),
    ] {
        let at = |params: &Value| render_scene(blurred((96, 64), source(), kind, params.clone()));
        let base = at(&params);
        assert_eq!(base.rgba, at(&params).rgba, "{kind} is not deterministic");
        let jump = (base.rgba.iter().zip(&at(&stepped).rgba))
            .map(|(a, b)| a.abs_diff(*b))
            .max();
        assert!(jump <= Some(5), "{kind}: a small step jumps {jump:?}");
    }
}

/// Sources sized to their bounds, grown by each blur's reach, draw what full-frame sources draw.
#[test]
fn sweep_bounds_match_reference() {
    let canvas = (384, 288);
    let small = |position: Value| {
        json!({ "id": 1, "type": "shape", "params": {
            "position": position, "width": 23, "height": 17, "fill_paint": [255, 120, 0, 220] } })
    };
    let mut cases = Vec::new();
    for edge in ["clamp", "transparent"] {
        for position in [json!([2, 3]), json!([361, 271]), json!([180.5, 140.25])] {
            for (kind, params) in [
                ("directional_blur", json!({ "angle": 0, "distance": 7 })),
                ("directional_blur", json!({ "angle": 63, "distance": 140 })),
                ("radial_blur", json!({ "center": [190, 150], "amount": 25 })),
                ("radial_blur", json!({ "center": [-200, 600], "amount": 4 })),
                ("radial_blur", json!({ "center": [10, 10], "amount": 360 })),
                ("zoom_blur", json!({ "center": [192, 144], "amount": 0.4 })),
                ("zoom_blur", json!({ "center": [380, 5], "amount": 2.5 })),
            ] {
                let mut params = params;
                params["edge"] = json!(edge);
                // A full turn about a corner sweeps over the whole canvas.
                let bounded = params["amount"] != json!(360);
                let case = Case::new(
                    format!("{kind} {params} at {position}"),
                    blurred(canvas, small(position.clone()), kind, params),
                    Expect::Nonblank,
                );
                cases.push(if bounded { case.bounded(true) } else { case });
            }
        }
    }
    let animated = |kind: &str, params: Value| {
        Case::new(
            format!("animated {kind}"),
            blurred(
                canvas,
                small(json!("=vec2(30 + frame * 40, 50 + frame * 20)")),
                kind,
                params,
            )
            .frames(6),
            Expect::Nonblank,
        )
        .frames([0.0, 2.0, 5.0])
        .bounded(true)
    };
    cases.push(animated(
        "directional_blur",
        json!({ "angle": "=frame * 25", "distance": "=3 + frame * 30" }),
    ));
    cases.push(animated(
        "radial_blur",
        json!({ "center": "=vec2(150, 100 + frame * 10)", "amount": "=frame * 12" }),
    ));
    cases.push(animated(
        "zoom_blur",
        json!({ "center": [200, 150], "amount": "=frame * 0.15", "edge": "transparent" }),
    ));
    let noise =
        json!({ "id": 1, "type": "fractal_noise", "params": { "scale": 25, "contrast": 2 } });
    for (kind, params) in [
        ("directional_blur", json!({ "angle": 20, "distance": 60 })),
        ("radial_blur", json!({ "center": [100, 200], "amount": 15 })),
        (
            "zoom_blur",
            json!({ "center": [200, 100], "amount": 0.3, "edge": "transparent" }),
        ),
    ] {
        cases.push(
            Case::new(
                format!("{kind} of a full-frame source"),
                blurred(canvas, noise.clone(), kind, params),
                Expect::Nonblank,
            )
            .bounded(false),
        );
    }
    check(cases);
}
