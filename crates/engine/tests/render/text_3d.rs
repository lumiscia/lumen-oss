//! `text_3d`: extruded text drawn by a `render_3d`.

use lumen_engine::{composition::Composition, gpu::RenderTarget};
use serde_json::{Value, json};

use crate::support::{Case, Expect, Frame, TestMedia, check, gpu, render, scene};

const SIZE: (u32, u32) = (64, 64);
const GREEN: [u8; 4] = [0, 255, 0, 255];

/// A `text_3d` with `params`, unlit green so that it shows its colour wherever it is drawn, under
/// the default camera.
fn text_3d(params: Value) -> Composition {
    let mut params = params;
    params["color"] = json!(GREEN);
    params["unlit"] = json!(true);
    scene(
        SIZE,
        json!([
            { "id": 1, "type": "text_3d", "params": params },
            { "id": 2, "type": "render_3d", "in": { "scene": 1 } },
        ]),
    )
    .into()
}

fn opaque(frame: &Frame) -> Vec<(u32, u32)> {
    (0..frame.height)
        .flat_map(|y| (0..frame.width).map(move |x| (x, y)))
        .filter(|&(x, y)| frame.pixel(x, y)[3] == 255)
        .collect()
}

#[test]
fn an_o_is_a_ring_with_its_counter_open_and_depth_shows_from_the_side() {
    let ring = |frames: &[Frame]| {
        let frame = &frames[0];
        let covered = opaque(frame);
        assert!(!covered.is_empty(), "the `o` is drawn");
        let (xs, ys): (Vec<u32>, Vec<u32>) = covered.into_iter().unzip();
        let (left, right) = (*xs.iter().min().unwrap(), *xs.iter().max().unwrap());
        let (top, bottom) = (*ys.iter().min().unwrap(), *ys.iter().max().unwrap());
        assert!(right - left >= 16, "the `o` is {left}..={right} wide");
        let (x, y) = ((left + right) / 2, (top + bottom) / 2);
        assert_eq!(
            frame.pixel(x, y)[3],
            0,
            "the counter at ({x}, {y}) is clear"
        );
        let row: String = (left..=right)
            .map(|x| {
                if frame.pixel(x, y)[3] >= 128 {
                    '#'
                } else {
                    '.'
                }
            })
            .collect();
        let strokes = row.split('.').filter(|run| !run.is_empty()).count();
        assert_eq!(strokes, 2, "the middle row crosses two strokes: {row}");
    };
    check(vec![
        Case::new(
            "an `o` facing the camera",
            text_3d(json!({ "content": "o", "font_size": 3 })),
            Expect::Custom(Box::new(ring)),
        ),
        Case::new(
            "bevelled",
            text_3d(json!({ "content": "o", "font_size": 3, "bevel_size": 0.06 })),
            Expect::Custom(Box::new(ring)),
        ),
    ]);

    // Turned 70° away, a flat `l` is a sliver and a deep one shows its side.
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let media = TestMedia::default();
    let mut coverage = |depth: f64| {
        let composition = text_3d(json!({
            "content": "l", "font_size": 3, "depth": depth, "rotation": [70, 0, 0],
        }));
        opaque(&render(&mut renderer, &composition, 0.0, &media)).len()
    };
    let (flat, deep) = (coverage(0.0), coverage(1.0));
    assert!(deep > flat * 2, "{deep} pixels deep against {flat} flat");
}

#[test]
fn text_with_nothing_to_draw_renders_nothing() {
    check(
        ["", "   ", "\n\n", "Text"]
            .into_iter()
            .map(|content| {
                let font_size = if content == "Text" { 0 } else { 1 };
                Case::new(
                    format!("{content:?} at size {font_size}"),
                    text_3d(json!({ "content": content, "font_size": font_size })),
                    Expect::Uniform([0, 0, 0, 0]),
                )
            })
            .collect(),
    );
}

#[test]
fn text_past_the_glyph_limit_is_a_render_error() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    renderer.set_max_text_glyphs(4);
    let media = TestMedia::default();
    // Whitespace is not drawn, so it does not count toward the limit.
    render(
        &mut renderer,
        &text_3d(json!({ "content": "ab  cd" })),
        0.0,
        &media,
    );
    let error = renderer
        .render(
            &text_3d(json!({ "content": "abcde" })),
            0.0,
            &media,
            RenderTarget::Owned,
        )
        .expect_err("five glyphs are past the limit")
        .to_string();
    assert!(
        error.contains("text extrudes 5 glyphs, more than the limit of 4"),
        "{error}"
    );
}

#[test]
fn numbers_no_text_can_have_are_render_errors_naming_the_param() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let media = TestMedia::default();
    for (param, value) in [
        ("depth", "=1e40"),
        ("bevel_size", "=-frame"),
        ("font_size", "=1e300"),
        ("letter_spacing", "=1e12"),
    ] {
        let composition = text_3d(json!({ "content": "l", param: value }));
        let error = renderer
            .render(&composition, 1.0, &media, RenderTarget::Owned)
            .expect_err("the value is not a size any text has")
            .to_string();
        assert!(
            error.contains(&format!("`{param}` must be")),
            "{param}: {error}"
        );
        assert!(error.contains("node 1"), "{param}: {error}");
    }
}

#[test]
fn a_family_no_font_answers_to_falls_back_with_a_warning() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let media = TestMedia::default();
    let composition = text_3d(json!({ "content": "a", "font_family": "Missing" }));
    let frame = render(&mut renderer, &composition, 0.0, &media);
    assert!(
        !opaque(&frame).is_empty(),
        "the text draws in the fallback font"
    );
    let warnings = renderer.take_diagnostics();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].code, "font.fallback");
    assert_eq!(warnings[0].node, Some(lumen_engine::node::NodeId::new(1)));
}
