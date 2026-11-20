//! `text_2d`: glyph outlines styled by the drawing nodes and drawn by a `render_2d`.

use lumen_engine::{composition::Composition, gpu::RenderTarget};
use serde_json::{Value, json};

use crate::support::{Frame, TestMedia, gpu, render, scene};

const INTER: &[u8] = include_bytes!("../../../text/assets/inter/InterVariable-Latin.ttf");
const SIZE: (u32, u32) = (320, 96);

fn media() -> TestMedia {
    TestMedia::default().font("Inter", "text-inter", INTER.to_vec())
}

/// `text` filled white by a `fill_2d`, with the `text_2d` params `text`.
fn filled(text: Value) -> Value {
    json!([
        { "id": 1, "type": "text_2d", "params": text },
        { "id": 2, "type": "fill_2d", "params": { "paint": [255, 255, 255, 255] }, "in": { "geometry": 1 } },
        { "id": 3, "type": "render_2d", "in": { "drawing": 2 } },
    ])
}

/// Each pixel's alpha.
fn coverage(frame: &Frame) -> Vec<f64> {
    frame
        .rgba
        .chunks(4)
        .map(|pixel| f64::from(pixel[3]) / 255.0)
        .collect()
}

#[test]
fn filled_text_covers_what_a_text_node_does() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let media = media();
    // Inter's variable outlines overlap where strokes join, as in `E`, `H` and `A`, which the
    // even-odd rule would leave as holes.
    for (family, content) in [("Roboto", "Lumen gyre"), ("Inter", "HEAT kfx")] {
        let text = json!({
            "content": content, "font_family": family, "font_size": 64, "position": [12, 10],
        });
        let outlined = render(
            &mut renderer,
            &scene(SIZE, filled(text.clone())).into(),
            0.0,
            &media,
        );
        let mut params = text;
        params["color"] = json!([255, 255, 255, 255]);
        let reference = render(
            &mut renderer,
            &scene(SIZE, json!([{ "id": 1, "type": "text", "params": params }])).into(),
            0.0,
            &media,
        );
        let (outlined, reference) = (coverage(&outlined), coverage(&reference));
        let total: f64 = reference.iter().sum();
        let differing: f64 = outlined
            .iter()
            .zip(&reference)
            .map(|(a, b)| (a - b).abs())
            .sum();
        let worst = outlined
            .iter()
            .zip(&reference)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f64::max);
        // Antialiasing differs along the edges; a glyph out of place, or a hole, differs by
        // whole pixels.
        assert!(total > 2000.0, "{family}: the text covers {total} pixels");
        assert!(
            differing < 0.04 * total,
            "{family}: coverage differs by {differing} of {total} pixels"
        );
        assert!(worst < 0.6, "{family}: a pixel differs by {worst}");
    }
}

#[test]
fn one_text_feeds_several_styles_and_is_shaped_once() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let media = TestMedia::default();
    let composition = |content: &str| -> Composition {
        scene(
            SIZE,
            json!([
                { "id": 1, "type": "text_2d", "params": { "content": content, "font_size": 72, "position": [12, 8] } },
                { "id": 2, "type": "fill_2d", "params": { "paint": [255, 255, 255, 255] }, "in": { "geometry": 1 } },
                { "id": 3, "type": "stroke_2d", "params": { "paint": [255, 0, 0, 255], "width": "=2 + frame" }, "in": { "geometry": 1 } },
                { "id": 4, "type": "stroke_2d", "params": { "paint": [0, 0, 255, 255], "width": 3, "dash": [6, 3] }, "in": { "geometry": 5 } },
                { "id": 5, "type": "offset_2d", "params": { "offset": 6 }, "in": { "geometry": 1 } },
                { "id": 6, "type": "merge_2d", "in": { "drawings": [4, 2, 3] } },
                { "id": 7, "type": "render_2d", "in": { "drawing": 6 } },
            ]),
        )
        .frames(4)
        .into()
    };
    let title = composition("Title");
    for at in [0.0, 1.0, 2.0] {
        let frame = render(&mut renderer, &title, at, &media);
        let red = frame
            .rgba
            .chunks(4)
            .filter(|pixel| pixel[0] > 200 && pixel[1] < 60)
            .count();
        let blue = frame
            .rgba
            .chunks(4)
            .filter(|pixel| pixel[2] > 200 && pixel[0] < 60)
            .count();
        assert!(
            red > 100 && blue > 100,
            "frame {at}: {red} red, {blue} blue"
        );
    }
    assert_eq!(renderer.text_2d_shapes(), 1, "the text is shaped once");
    render(&mut renderer, &composition("Titles"), 0.0, &media);
    assert_eq!(renderer.text_2d_shapes(), 2, "new text is shaped again");
}

#[test]
fn text_past_its_limits_is_a_render_error() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    renderer.set_max_text_glyphs(4);
    let media = TestMedia::default();
    let text = |params: Value| -> Composition { scene(SIZE, filled(params)).into() };
    // Whitespace is not drawn, so it does not count toward the limit.
    render(
        &mut renderer,
        &text(json!({ "content": "ab  cd" })),
        0.0,
        &media,
    );
    let mut error = |params: Value| {
        renderer
            .render(&text(params), 0.0, &media, RenderTarget::Owned)
            .expect_err("the text is past a limit")
            .to_string()
    };
    let glyphs = error(json!({ "content": "abcde" }));
    assert!(
        glyphs.contains("text_2d node 1")
            && glyphs.contains("text outlines 5 glyphs, more than the limit of 4"),
        "{glyphs}"
    );
    let size = error(json!({ "content": "a", "font_size": "=1e300" }));
    assert!(
        size.contains("text_2d node 1") && size.contains("`font_size` must be at most"),
        "{size}"
    );
}
