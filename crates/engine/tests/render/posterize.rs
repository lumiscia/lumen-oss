//! `posterize` on a grey ramp, where each step and band is known from its parameters.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::support::{Case, Expect, Frame, TestMedia, check, gpu, render, scene};

/// `posterize` with `params` over a ramp of encoded grey 0 to 255 across `rows` rows.
fn ramp(rows: u32, params: Value) -> Frame {
    let rgba = (0..rows)
        .flat_map(|_| (0..=255u8).flat_map(|value| [value, value, value, 255]))
        .collect();
    let media = TestMedia::default().image("ramp", 256, rows, rgba);
    let composition = scene(
        (256, rows),
        json!([
            { "id": 1, "type": "media_in", "params": { "kind": "image", "source": "ramp" } },
            { "id": 2, "type": "posterize", "in": { "source": 1 }, "params": params },
        ]),
    )
    .into();
    render(&mut gpu().renderer(), &composition, 0.0, &media)
}

fn colors(frame: &Frame, y: u32) -> BTreeSet<[u8; 3]> {
    (0..frame.width)
        .map(|x| {
            let [r, g, b, _] = frame.pixel(x, y);
            [r, g, b]
        })
        .collect()
}

#[test]
fn levels_snap_each_channel_to_evenly_spaced_encoded_steps() {
    let two = ramp(1, json!({ "levels": 2 }));
    assert_eq!(colors(&two, 0), BTreeSet::from([[0; 3], [255; 3]]));
    assert_eq!(two.pixel(127, 0)[0], 0);
    assert_eq!(two.pixel(128, 0)[0], 255);

    let four = ramp(1, json!({ "levels": 4 }));
    assert_eq!(
        colors(&four, 0),
        BTreeSet::from([[0; 3], [85; 3], [170; 3], [255; 3]])
    );
}

#[test]
fn a_palette_maps_brightness_bands_onto_its_colours_in_luma_order() {
    // Listed out of order. Their lumas are about 0.04, 0.26 and 0.88, so the bands split at
    // grey 38 and 145.
    let dark = [10, 10, 10];
    let red = [200, 30, 30];
    let ash = [220, 225, 230];
    let frame = ramp(1, json!({ "mode": "palette", "colors": [ash, dark, red] }));
    assert_eq!(colors(&frame, 0), BTreeSet::from([dark, red, ash]));
    for (x, expected) in [
        (0, dark),
        (30, dark),
        (50, red),
        (130, red),
        (160, ash),
        (255, ash),
    ] {
        let [r, g, b, _] = frame.pixel(x, 0);
        assert_eq!([r, g, b], expected, "grey {x}");
    }
}

#[test]
fn dither_turns_a_band_edge_into_a_pattern_of_only_palette_colours() {
    let palette = json!({ "mode": "palette", "colors": [[0, 0, 0], [255, 255, 255]] });
    let mixed = |frame: &Frame| {
        (0..frame.width)
            .filter(|&x| (0..frame.height).any(|y| frame.pixel(x, y)[0] != frame.pixel(x, 0)[0]))
            .count()
    };

    let hard = ramp(8, palette.clone());
    assert_eq!(mixed(&hard), 0);

    let mut params = palette;
    params["dither"] = json!(1);
    let dithered = ramp(8, params);
    for y in 0..8 {
        assert_eq!(colors(&dithered, y), BTreeSet::from([[0; 3], [255; 3]]));
    }
    assert!(mixed(&dithered) > 100, "{} columns mix", mixed(&dithered));
}

#[test]
fn alpha_is_kept() {
    let half_red = [255, 0, 0, 128];
    let premultiplied = [128, 0, 0, 128];
    let over = |params: Value| {
        scene(
            (4, 4),
            json!([
                { "id": 1, "type": "background", "params": { "paint": half_red } },
                { "id": 2, "type": "posterize", "in": { "source": 1 }, "params": params },
            ]),
        )
    };
    check(vec![
        Case::new(
            "levels",
            over(json!({ "levels": 2 })),
            Expect::Uniform(premultiplied),
        ),
        Case::new(
            "palette",
            over(json!({ "mode": "palette", "colors": [[255, 0, 0], [255, 255, 255]] })),
            Expect::Uniform(premultiplied),
        ),
    ]);
}
