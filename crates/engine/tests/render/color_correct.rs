//! `color_correct`: each stage on flat colour, with expected values worked out from its formula.

use serde_json::{Value, json};

use crate::color::{linear, opaque};
use crate::support::{Case, Expect, Scene, check, scene};

/// `color_correct` with `params` over a flat opaque `paint`.
fn correct(paint: [u8; 4], params: Value) -> Scene {
    scene(
        (4, 4),
        json!([
            { "id": 1, "type": "background", "params": { "paint": paint } },
            { "id": 2, "type": "color_correct", "in": { "source": 1 }, "params": params },
        ]),
    )
}

#[test]
fn defaults_leave_colour_unchanged() {
    let paint = [200, 80, 40, 255];
    check(vec![Case::new(
        "default params",
        correct(paint, json!({})),
        Expect::Uniform(paint),
    )]);
}

#[test]
fn gain_scales_light_and_warmth_keeps_the_luminance_of_grey() {
    let grey = linear(128);
    let factors = [0.5_f64.exp2(), 1.0, (-0.5_f64).exp2()];
    let luminance = 0.2126 * factors[0] + 0.7152 * factors[1] + 0.0722 * factors[2];
    let warmed = factors.map(|factor| grey * factor / luminance);
    check(vec![
        Case::new(
            "gain doubles linear red",
            correct([128, 128, 128, 255], json!({ "gain": [2, 1, 1] })),
            Expect::Uniform(opaque([grey * 2.0, grey, grey])),
        ),
        Case::new(
            "temperature 1 warms grey at the same luminance",
            correct([128, 128, 128, 255], json!({ "temperature": 1 })),
            Expect::Uniform(opaque(warmed)),
        ),
    ]);
}

#[test]
fn a_master_curve_maps_encoded_mid_grey_through_its_points() {
    // Code 128 sits just above the middle point, whose slope is 1.
    check(vec![
        Case::new(
            "master curve through (0.5, 0.75)",
            correct(
                [128, 128, 128, 255],
                json!({ "curves": { "master": [[0, 0], [0.5, 0.75], [1, 1]] } }),
            ),
            Expect::Uniform([192, 192, 192, 255]),
        ),
        Case::new(
            "a red curve leaves green and blue",
            correct(
                [128, 128, 128, 255],
                json!({ "curves": { "red": [[0, 0], [0.5, 0.75], [1, 1]] } }),
            ),
            Expect::Uniform([192, 128, 128, 255]),
        ),
    ]);
}
