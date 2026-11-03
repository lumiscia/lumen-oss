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
