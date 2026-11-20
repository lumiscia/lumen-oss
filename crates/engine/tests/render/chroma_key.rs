//! `chroma_key`: the matte on flat colour and on a gradient, and spill suppression.

use serde_json::{Value, json};

use crate::color::{linear, opaque};
use crate::support::{Case, Expect, Scene, TestMedia, check, gpu, render, scene};

const GREEN: [u8; 4] = [0, 255, 0, 255];

/// `chroma_key` with `params` over a flat opaque `paint`.
fn key(paint: [u8; 4], params: Value) -> Scene {
    scene(
        (4, 4),
        json!([
            { "id": 1, "type": "background", "params": { "paint": paint } },
            { "id": 2, "type": "chroma_key", "in": { "source": 1 }, "params": params },
        ]),
    )
}

#[test]
fn the_key_colour_is_removed_and_other_colours_are_kept() {
    check(vec![
        Case::new(
            "green",
            key(GREEN, json!({})),
            Expect::Uniform([0, 0, 0, 0]),
        ),
        Case::new(
            "red",
            key([255, 0, 0, 255], json!({})),
            Expect::Uniform([255, 0, 0, 255]),
        ),
    ]);
}

#[test]
fn dark_and_bright_green_key_alike() {
    // A row of greens from just above the near-black floor up to full green.
    let pixels = (0..64u32)
        .flat_map(|x| [0, 10 + (x * 245 / 63) as u8, 0, 255])
        .collect();
    let media = TestMedia::default().image("plate", 64, 1, pixels);
    let nodes = json!([
        { "id": 1, "type": "media_in", "params": { "kind": "image", "source": "plate" } },
        { "id": 2, "type": "chroma_key", "in": { "source": 1 } },
    ]);
    check(vec![
        Case::new(
            "green gradient",
            scene((64, 1), nodes),
            Expect::Uniform([0, 0, 0, 0]),
        )
        .media(media),
    ]);
}

#[test]
fn spill_limits_the_key_channel_of_the_pixels_that_are_kept() {
    // Linear (0.3, 0.45, 0.3), as sRGB codes: far enough from green to be kept, but with a cast.
    let paint = [149, 179, 149, 255];
    let grey = linear(149);
    check(vec![
        Case::new(
            "spill 1 lowers green to red and blue",
            key(paint, json!({ "spill": 1 })),
            Expect::Uniform(opaque([grey, grey, grey])),
        ),
        Case::new(
            "spill 0 leaves it",
            key(paint, json!({ "spill": 0 })),
            Expect::Uniform(paint),
        ),
    ]);
}

#[test]
fn matte_only_is_white_with_the_alpha_of_the_keyed_image() {
    // Between the tolerance and the softness, so the alpha is partial.
    let params = json!({ "tolerance": 0.2, "softness": 0.2 });
    let render_pixel = |matte_only: bool| {
        let mut params = params.clone();
        params["matte_only"] = json!(matte_only);
        let composition = key([90, 200, 90, 255], params).into();
        let gpu = gpu();
        render(
            &mut gpu.renderer(),
            &composition,
            0.0,
            &TestMedia::default(),
        )
        .pixel(0, 0)
    };
    let keyed = render_pixel(false);
    let matte = render_pixel(true);
    assert!(
        (1..255).contains(&keyed[3]),
        "alpha {} is not partial",
        keyed[3]
    );
    assert_eq!(matte, [keyed[3]; 4]);
}
