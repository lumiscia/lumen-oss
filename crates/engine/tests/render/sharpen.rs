//! Sharpen against its definition: it leaves flat colour, amount 0 and detail under the threshold
//! alone, holds hard edges without halos, and steepens soft ones.

use lumen_engine::composition::Composition;
use serde_json::{Value, json};

use crate::support::{Case, Expect, Frame, Scene, TestMedia, check, gpu, render, scene};

const CANVAS: (u32, u32) = (64, 8);

/// A horizontal ramp from `from` to `to` grey between the canvas fractions `start` and `end`.
fn ramp(from: u8, to: u8, start: f64, end: f64) -> Value {
    json!({ "id": 1, "type": "background", "params": { "paint": {
        "type": "linear_gradient", "start": [0, 0], "end": [1, 0],
        "stops": [
            [0, [from, from, from, 255]], [start, [from, from, from, 255]],
            [end, [to, to, to, 255]], [1, [to, to, to, 255]],
        ] }, "paint_supersample": false } })
}

fn sharpened(source: Value, params: Value) -> Scene {
    scene(
        CANVAS,
        json!([source, { "id": 2, "type": "sharpen", "params": params, "in": { "source": 1 } }]),
    )
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

/// Whether no byte differs by more than the rounding of a colour through the working format.
fn within_one(pixels: &[u8], reference: &[u8]) -> Result<(), String> {
    match pixels
        .iter()
        .zip(reference)
        .position(|(a, b)| a.abs_diff(*b) > 1)
    {
        Some(index) => Err(format!(
            "byte {index} is {}, expected {}",
            pixels[index], reference[index]
        )),
        None => Ok(()),
    }
}

fn row(frame: &Frame) -> Vec<u8> {
    (0..frame.width).map(|x| frame.pixel(x, 4)[0]).collect()
}

fn steepest(row: &[u8]) -> u8 {
    row.windows(2)
        .map(|pair| pair[0].abs_diff(pair[1]))
        .max()
        .unwrap()
}

#[test]
fn nothing_to_sharpen_is_unchanged() {
    let soft = || ramp(0, 255, 0.25, 0.75);
    let source = render_scene(scene(CANVAS, json!([soft()])));
    let untouched = |params: Value| {
        let frame = render_scene(sharpened(soft(), params));
        within_one(&frame.rgba, &source.rgba)
    };
    assert_eq!(untouched(json!({ "amount": 0, "radius": 3 })), Ok(()));
    assert_eq!(untouched(json!({ "amount": 2, "radius": 0 })), Ok(()));
    // The ramp's detail is far under the threshold.
    assert_eq!(
        untouched(json!({ "amount": 4, "radius": 3, "threshold": 0.5 })),
        Ok(())
    );
    assert_ne!(
        render_scene(sharpened(soft(), json!({ "amount": 4, "radius": 3 }))).rgba,
        source.rgba
    );
    check(vec![Case::new(
        "flat colour has no detail",
        sharpened(
            json!({ "id": 1, "type": "background", "params": { "paint": [200, 100, 50, 255] } }),
            json!({ "amount": 5, "radius": 4 }),
        ),
        Expect::Uniform([200, 100, 50, 255]),
    )]);
}

/// A step between two greys, sharpened hard, stays between them: no pixel overshoots.
#[test]
fn hard_edges_get_no_halos() {
    let step = || ramp(64, 192, 0.5, 0.5);
    let source = render_scene(scene(CANVAS, json!([step()])));
    let frame = render_scene(sharpened(step(), json!({ "amount": 10, "radius": 3 })));
    assert_eq!(within_one(&frame.rgba, &source.rgba), Ok(()));
}

/// A soft edge's pixels move apart, towards their neighbours' values and no further.
#[test]
fn soft_edges_get_steeper() {
    let soft = || ramp(0, 255, 0.4, 0.6);
    let before = row(&render_scene(scene(CANVAS, json!([soft()]))));
    let after = row(&render_scene(sharpened(
        soft(),
        json!({ "amount": 2, "radius": 2 }),
    )));
    assert!(
        steepest(&after) > steepest(&before) * 6 / 5,
        "{} after, {} before",
        steepest(&after),
        steepest(&before)
    );
    assert!(after[0] == 0 && after[63] == 255);
}

/// An image smaller than the canvas is sharpened as it would be on a full canvas: the blur reads
/// the transparent canvas past the image's edge rather than clamping to it.
#[test]
fn bounded_sources_sharpen_like_full_canvas_ones() {
    const SIDE: u32 = 16;
    let rgba = (0..SIDE * SIDE)
        .flat_map(|index| {
            let grey = match index % SIDE {
                0 => 128,
                1 => 0,
                _ => 255,
            };
            [grey, grey, grey, 255]
        })
        .collect();
    let media = TestMedia::default().image("tile", SIDE, SIDE, rgba);
    let composition: Composition = scene(
        (64, 32),
        json!([
            { "id": 1, "type": "media_in", "params": { "kind": "image", "source": "tile" } },
            { "id": 2, "type": "transform", "in": { "source": 1 },
              "params": { "translate_x": 8, "translate_y": 4, "sampling": "nearest" } },
            { "id": 3, "type": "sharpen", "in": { "source": 2 },
              "params": { "amount": 1, "radius": 2 } }
        ]),
    )
    .into();
    let gpu = gpu();
    let shown = |full_frame: bool| {
        let mut renderer = gpu.renderer();
        renderer.set_full_frame_sources(full_frame);
        render(&mut renderer, &composition, 0.0, &media)
    };
    assert_eq!(within_one(&shown(false).rgba, &shown(true).rgba), Ok(()));
}
