//! `halftone` and `ordered_dither` on flat colour, where each pattern is known in closed form.

use lumen_engine::composition::Composition;
use serde_json::{Value, json};

use crate::color::decode;
use crate::support::{Case, Expect, Scene, TestMedia, check, gpu, render, scene};

/// `kind` with `params` over a flat `paint`.
fn through(kind: &str, canvas: u32, paint: [u8; 4], params: Value) -> Scene {
    scene(
        (canvas, canvas),
        json!([
            { "id": 1, "type": "background", "params": { "paint": paint } },
            { "id": 2, "type": kind, "in": { "source": 1 }, "params": params },
        ]),
    )
}

#[test]
fn a_flat_colour_gives_dots_of_one_size_on_a_rotated_grid() {
    const PITCH: f64 = 16.0;
    const ANGLE: f64 = 30.0;
    let composition: Composition = through(
        "halftone",
        128,
        [128, 128, 128, 255],
        json!({ "pitch": PITCH, "angle": ANGLE }),
    )
    .into();
    let gpu = gpu();
    let frame = render(
        &mut gpu.renderer(),
        &composition,
        0.0,
        &TestMedia::default(),
    );

    let (sin, cos) = ANGLE.to_radians().sin_cos();
    let mut ink = std::collections::BTreeMap::<(i64, i64), f64>::new();
    for y in 0..frame.height {
        for x in 0..frame.width {
            let (px, py) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
            let cell = (
                ((px * cos + py * sin) / PITCH).floor() as i64,
                ((-px * sin + py * cos) / PITCH).floor() as i64,
            );
            let paper = decode(f64::from(frame.pixel(x, y)[0]) / 255.0);
            *ink.entry(cell).or_default() += 1.0 - paper;
        }
    }
    // Cells that the canvas edge or its corners cut are left out.
    let whole: Vec<f64> = ink
        .iter()
        .filter(|((u, v), _)| {
            let centre = ((*u as f64 + 0.5) * PITCH, (*v as f64 + 0.5) * PITCH);
            let (x, y) = (
                centre.0 * cos - centre.1 * sin,
                centre.0 * sin + centre.1 * cos,
            );
            (24.0..104.0).contains(&x) && (24.0..104.0).contains(&y)
        })
        .map(|(_, coverage)| *coverage)
        .collect();
    assert!(whole.len() >= 8, "only {} whole cells", whole.len());
    // Half of an encoded 128 grey's darkness, over a cell of 256 pixels.
    let expected = (1.0 - 128.0 / 255.0) * PITCH * PITCH;
    for coverage in whole {
        assert!(
            (coverage - expected).abs() < expected * 0.03,
            "a dot covers {coverage} pixels, expected {expected}"
        );
    }
}

/// The mean ink coverage of whole cells of a flat grey `value` halftone.
fn grey_coverage(value: u8) -> f64 {
    let composition: Composition = through(
        "halftone",
        128,
        [value, value, value, 255],
        json!({ "pitch": 16, "angle": 0 }),
    )
    .into();
    let gpu = gpu();
    let frame = render(
        &mut gpu.renderer(),
        &composition,
        0.0,
        &TestMedia::default(),
    );
    let mut total = 0.0;
    for y in 16..112 {
        for x in 16..112 {
            total += 1.0 - decode(f64::from(frame.pixel(x, y)[0]) / 255.0);
        }
    }
    total / (96.0 * 96.0)
}

#[test]
fn dark_tones_keep_their_area_and_only_the_last_percent_closes_to_solid() {
    // Encoded greys of darkness about 0.9, 0.95, 0.99 and 1.
    let tones = [(26u8, 0.9), (13, 0.95), (3, 0.99), (0, 1.0)];
    let coverages: Vec<f64> = tones
        .iter()
        .map(|(value, _)| grey_coverage(*value))
        .collect();
    for ((value, darkness), coverage) in tones.iter().zip(&coverages) {
        let expected = 1.0 - f64::from(*value) / 255.0;
        assert!(
            (coverage - expected).abs() < 0.02,
            "grey {value} (darkness {darkness}) covers {coverage}, expected {expected}"
        );
    }
    assert!(
        coverages.windows(2).all(|pair| pair[0] < pair[1]),
        "coverage is not increasing: {coverages:?}"
    );
}

#[test]
fn black_fills_the_cell_and_white_leaves_the_paper() {
    let halftone = |paint, angle| {
        through(
            "halftone",
            40,
            paint,
            json!({ "pitch": 10, "angle": angle }),
        )
    };
    check(vec![
        Case::new(
            "black",
            halftone([0, 0, 0, 255], 0),
            Expect::Uniform([0, 0, 0, 255]),
        ),
        Case::new(
            "black on a rotated grid",
            halftone([0, 0, 0, 255], 33),
            Expect::Uniform([0, 0, 0, 255]),
        ),
        Case::new(
            "white",
            halftone([255, 255, 255, 255], 0),
            Expect::Uniform([255, 255, 255, 255]),
        ),
    ]);
}

#[test]
fn two_by_two_dither_of_mid_grey_is_a_checker() {
    let ink = [0, 0, 0, 255];
    let paper = [255, 255, 255, 255];
    let probes = (0..4)
        .flat_map(|y| (0..4).map(move |x| (x, y)))
        .map(|(x, y)| (x, y, if (x + y) % 2 == 1 { ink } else { paper }))
        .collect();
    check(vec![Case::new(
        "bayer2, 2 levels",
        through(
            "ordered_dither",
            4,
            [128, 128, 128, 255],
            json!({ "matrix": "bayer2", "levels": 2, "pitch": 1 }),
        ),
        Expect::Probes(probes),
    )]);
}

#[test]
fn both_nodes_keep_the_source_alpha() {
    let half_white = [255, 255, 255, 128];
    check(vec![
        Case::new(
            "halftone",
            through("halftone", 8, half_white, json!({})),
            Expect::Uniform([128, 128, 128, 128]),
        ),
        Case::new(
            "ordered_dither",
            through("ordered_dither", 8, half_white, json!({})),
            Expect::Uniform([128, 128, 128, 128]),
        ),
    ]);
}
