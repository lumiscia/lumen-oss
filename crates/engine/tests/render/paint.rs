//! Gradients drawn by `background`, with a black-to-white ramp so a pixel's value is its position
//! along the gradient.

use serde_json::{Value, json};

use crate::support::{Case, Expect, check, scene};

fn grey(value: u8) -> [u8; 4] {
    [value, value, value, 255]
}

fn ramp(width: u32, height: u32, mut paint: Value) -> crate::support::Scene {
    paint["stops"] = json!([
        { "offset": 0, "color": [0, 0, 0, 255] },
        { "offset": 1, "color": [255, 255, 255, 255] },
    ]);
    background(width, height, paint)
}

fn background(width: u32, height: u32, paint: Value) -> crate::support::Scene {
    scene(
        (width, height),
        json!([{ "id": 1, "type": "background", "params": {
            "paint": paint, "width": width, "height": height, "paint_supersample": false
        } }]),
    )
}

/// A row of `x` positions on a 16 × 1 canvas, where the gradient reaches 1 at x = 8.
fn along_the_row(paint: Value, values: [u8; 5]) -> (crate::support::Scene, Expect) {
    let probes = [0, 4, 8, 12, 15]
        .into_iter()
        .zip(values)
        .map(|(x, value)| (x, 0, grey(value)))
        .collect();
    (ramp(16, 1, paint), Expect::Probes(probes))
}

#[test]
fn spread_modes_continue_a_gradient_past_its_end() {
    let spreads = [
        ("pad", [16, 143, 255, 255, 255]),
        ("repeat", [16, 143, 16, 143, 239]),
        ("reflect", [16, 143, 239, 112, 16]),
    ];
    let mut cases = Vec::new();
    for (spread, values) in spreads {
        let linear = json!({
            "type": "linear_gradient", "start": [0, 0], "end": [0.5, 0], "spread": spread
        });
        let radial = json!({
            "type": "radial_gradient", "center": [0, 0.5], "radius": [0.5, 0.5], "spread": spread
        });
        let user_space = json!({
            "type": "linear_gradient", "units": "user_space", "start": [0, 0], "end": [8, 0],
            "spread": spread
        });
        for (kind, paint) in [
            ("linear", linear),
            ("radial", radial),
            ("user space", user_space),
        ] {
            let (scene, expect) = along_the_row(paint, values);
            cases.push(Case::new(format!("{kind} {spread}"), scene, expect).tolerance(2));
        }
    }
    check(cases);
}

#[test]
fn conic_gradients_sweep_clockwise_from_the_left_and_ignore_the_spread() {
    let probes = |values: [u8; 6]| {
        let positions = [(3, 2), (2, 3), (0, 1), (1, 0), (3, 1), (0, 3)];
        Expect::Probes(
            positions
                .into_iter()
                .zip(values)
                .map(|((x, y), value)| (x, y, grey(value)))
                .collect(),
        )
    };
    let conic = |extra: Value| {
        let mut paint = json!({ "type": "conic_gradient", "center": [0.5, 0.5] });
        paint
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        paint
    };
    let mut cases = Vec::new();
    for spread in ["pad", "repeat", "reflect"] {
        cases.push(
            Case::new(
                format!("conic {spread}"),
                ramp(4, 4, conic(json!({ "spread": spread }))),
                probes([141, 178, 13, 51, 114, 223]),
            )
            .tolerance(2),
        );
    }
    cases.push(
        Case::new(
            "conic rotated a quarter turn",
            ramp(4, 4, conic(json!({ "angle": 90 }))),
            probes([77, 114, 204, 242, 51, 159]),
        )
        .tolerance(2),
    );
    check(cases);
}

/// Seven black stops, then a white one at the end of a 16 × 1 linear gradient.
fn black_then_white(black_stops: usize) -> Value {
    let mut stops = vec![json!([1.0, [255, 255, 255]])];
    stops.extend((0..black_stops).map(|index| json!([index as f64 / 7.0, [0, 0, 0]])));
    json!({ "type": "linear_gradient", "stops": stops })
}

#[test]
fn a_gradient_uses_all_eight_stops_and_refuses_a_ninth() {
    check(vec![
        Case::new(
            "eight stops",
            background(16, 1, black_then_white(7)),
            Expect::Probes(vec![(0, 0, grey(0)), (15, 0, grey(199))]),
        )
        .tolerance(2),
    ]);

    let error = lumen_engine::json::parse_value(&background(16, 1, black_then_white(8)).0)
        .expect_err("nine stops must not load");
    let error = format!("{error:#}");
    assert!(
        error.contains("9 stops") && error.contains("at most 8"),
        "{error}"
    );
}

#[test]
fn linear_srgb_interpolation_blends_in_linear_light() {
    let paint = |interpolation: &str| {
        json!({
            "type": "linear_gradient", "start": [0, 0], "end": [1, 0],
            "interpolation": interpolation
        })
    };
    let translucent = json!({
        "type": "linear_gradient", "start": [0, 0], "end": [1, 0],
        "interpolation": "linear_srgb",
        "stops": [
            { "offset": 0, "color": [255, 255, 255, 255] },
            { "offset": 1, "color": [0, 0, 0, 0] },
        ]
    });
    let midpoint = |scene| Expect::Probes(vec![(8, 0, scene)]);
    check(vec![
        Case::new(
            "srgb",
            ramp(17, 1, paint("srgb")),
            midpoint([128, 128, 128, 255]),
        )
        .tolerance(2),
        Case::new(
            "linear_srgb",
            ramp(17, 1, paint("linear_srgb")),
            midpoint([188, 188, 188, 255]),
        )
        .tolerance(2),
        // Premultiplied linear white at half alpha is still white, so no dark fringe appears.
        Case::new(
            "linear_srgb to a transparent stop",
            background(17, 1, translucent),
            midpoint([128, 128, 128, 128]),
        )
        .tolerance(2),
    ]);
}
