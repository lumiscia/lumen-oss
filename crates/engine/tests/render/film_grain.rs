//! Film grain against its definition: strength measured on flat grey, nothing added where there is
//! nothing to grain, and a pattern fixed to the canvas that is the same at any render scale.

use lumen_engine::composition::Composition;
use serde_json::{Value, json};

use crate::support::{Case, Expect, Frame, Scene, TestMedia, check, gpu, render, scene};

const CANVAS: (u32, u32) = (256, 256);

fn grey() -> Value {
    json!({ "id": 1, "type": "background", "params": { "paint": [128, 128, 128, 255] } })
}

fn grained(canvas: (u32, u32), source: Value, params: Value) -> Scene {
    scene(
        canvas,
        json!([source, { "id": 2, "type": "film_grain", "params": params, "in": { "source": 1 } }]),
    )
}

fn render_scene(scene: Scene, scale: f32) -> Frame {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    renderer.set_render_scale(scale).expect("a valid scale");
    render(
        &mut renderer,
        &Composition::from(scene),
        0.0,
        &TestMedia::default(),
    )
}

/// The mean and standard deviation of one channel, in sRGB codes.
fn statistics(frame: &Frame, channel: usize) -> (f64, f64) {
    let values: Vec<f64> = frame
        .rgba
        .chunks(4)
        .map(|pixel| f64::from(pixel[channel]))
        .collect();
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64;
    (mean, variance.sqrt())
}

#[test]
fn amount_zero_returns_the_source_exactly() {
    let photo = TestMedia::default().image(
        "photo",
        16,
        16,
        (0..16 * 16)
            .flat_map(|i| {
                [
                    (i % 16 * 16) as u8,
                    (i / 16 * 16) as u8,
                    90,
                    (i * 7 % 256) as u8,
                ]
            })
            .collect(),
    );
    let source =
        json!({ "id": 1, "type": "media_in", "params": { "kind": "image", "source": "photo" } });
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let plain = scene((16, 16), json!([source.clone()]));
    let zero = grained((16, 16), source, json!({ "amount": 0, "roughness": 0.5 }));
    let expected = render(&mut renderer, &plain.into(), 0.0, &photo);
    let actual = render(&mut renderer, &zero.into(), 0.0, &photo);
    assert_eq!(actual.rgba, expected.rgba);
}

#[test]
fn grain_has_zero_mean_and_the_amount_as_its_deviation() {
    let amount = 0.1;
    let params =
        |monochrome: bool| json!({ "amount": amount, "response": 0, "monochrome": monochrome });
    let mono = render_scene(grained(CANVAS, grey(), params(true)), 1.0);
    let colour = render_scene(grained(CANVAS, grey(), params(false)), 1.0);
    let expected = amount * 255.0;
    for channel in 0..3 {
        for (name, frame) in [("monochrome", &mono), ("per channel", &colour)] {
            let (mean, deviation) = statistics(frame, channel);
            assert!((mean - 128.0).abs() < 1.0, "{name} {channel}: mean {mean}");
            assert!(
                (deviation / expected - 1.0).abs() < 0.05,
                "{name} {channel}: deviation {deviation}, expected {expected}"
            );
        }
    }
    assert!(
        mono.rgba.chunks(4).all(|p| p[0] == p[1] && p[1] == p[2]),
        "monochrome grain moves every channel alike"
    );
    assert!(
        colour.rgba.chunks(4).any(|p| p[0] != p[1]),
        "per-channel grain differs between channels"
    );
}

#[test]
fn grain_strength_does_not_depend_on_size_or_render_scale() {
    // Size 0.5 at scale 1 takes the subsampled path, and size 1 at scale 2 puts texel centres a
    // quarter cell from the lattice.
    for (size, scale) in [(0.5, 1.0), (1.0, 1.0), (1.5, 1.0), (1.0, 2.0)] {
        let params = json!({ "amount": 0.1, "size": size, "response": 0 });
        let frame = render_scene(grained(CANVAS, grey(), params), scale);
        let (_, deviation) = statistics(&frame, 0);
        assert!(
            (deviation / 25.5 - 1.0).abs() < 0.05,
            "size {size}, scale {scale}: deviation {deviation}"
        );
    }
}

#[test]
fn roughness_keeps_the_strength_and_coarsens_the_grain() {
    let params = |roughness: f64| json!({ "amount": 0.1, "response": 0, "roughness": roughness });
    let fine = render_scene(grained(CANVAS, grey(), params(0.0)), 1.0);
    let rough = render_scene(grained(CANVAS, grey(), params(1.0)), 1.0);
    let (_, fine_deviation) = statistics(&fine, 0);
    let (_, rough_deviation) = statistics(&rough, 0);
    assert!(
        (rough_deviation / fine_deviation - 1.0).abs() < 0.12,
        "deviations {fine_deviation} and {rough_deviation}"
    );
    // Neighbouring pixels agree far more in coarse grain than in fine grain.
    let step = |frame: &Frame| {
        let rows = frame.rgba.chunks(frame.width as usize * 4);
        let (sum, count) = rows.fold((0.0, 0.0), |(sum, count), row| {
            let pixels: Vec<f64> = row.chunks(4).map(|p| f64::from(p[0])).collect();
            let diffs: f64 = pixels.windows(2).map(|w| (w[1] - w[0]).abs()).sum();
            (sum + diffs, count + (pixels.len() - 1) as f64)
        });
        sum / count
    };
    assert!(
        step(&rough) * 2.0 < step(&fine),
        "{} {}",
        step(&rough),
        step(&fine)
    );
}

#[test]
fn full_response_leaves_black_and_white_unchanged() {
    let params = json!({ "amount": 0.3, "response": 1 });
    let background = |code: u8| json!({ "id": 1, "type": "background", "params": { "paint": [code, code, code, 255] } });
    check(vec![
        Case::new(
            "black",
            grained(CANVAS, background(0), params.clone()),
            Expect::Uniform([0, 0, 0, 255]),
        ),
        Case::new(
            "white",
            grained(CANVAS, background(255), params),
            Expect::Uniform([255, 255, 255, 255]),
        ),
    ]);
}

#[test]
fn seed_picks_the_pattern_and_frames_animate_it() {
    let at = |params: Value| render_scene(grained((64, 64), grey(), params), 1.0).rgba;
    let base = json!({ "amount": 0.2 });
    let with_seed = |seed: i64| json!({ "amount": 0.2, "seed": seed });
    assert_eq!(at(base.clone()), at(with_seed(0)));
    assert_eq!(at(with_seed(7)), at(with_seed(7)));
    assert_ne!(at(with_seed(7)), at(with_seed(8)));

    let animated = grained((64, 64), grey(), json!({ "amount": 0.2, "seed": "=frame" })).frames(2);
    let composition = Composition::from(animated);
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let media = TestMedia::default();
    let first = render(&mut renderer, &composition, 0.0, &media);
    let second = render(&mut renderer, &composition, 1.0, &media);
    assert_ne!(first.rgba, second.rgba);
}

/// The Pearson correlation of two equal-length series.
fn correlation(a: &[f64], b: &[f64]) -> f64 {
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let (ma, mb) = (mean(a), mean(b));
    let (mut cov, mut va, mut vb) = (0.0, 0.0, 0.0);
    for (a, b) in a.iter().zip(b) {
        cov += (a - ma) * (b - mb);
        va += (a - ma).powi(2);
        vb += (b - mb).powi(2);
    }
    cov / (va * vb).sqrt()
}

#[test]
fn grain_is_the_same_pattern_at_any_render_scale() {
    let params = json!({ "amount": 0.2, "size": 4, "response": 0 });
    let scene = || grained((64, 64), grey(), params.clone());
    let one = render_scene(scene(), 1.0);
    let two = render_scene(scene(), 2.0);
    assert_eq!((two.width, two.height), (128, 128));
    let (mut at_one, mut at_two) = (Vec::new(), Vec::new());
    for y in 0..64 {
        for x in 0..64 {
            at_one.push(f64::from(one.pixel(x, y)[0]));
            let block: f64 = [(0, 0), (1, 0), (0, 1), (1, 1)]
                .iter()
                .map(|(dx, dy)| f64::from(two.pixel(x * 2 + dx, y * 2 + dy)[0]))
                .sum();
            at_two.push(block / 4.0);
        }
    }
    let agreement = correlation(&at_one, &at_two);
    assert!(agreement > 0.95, "correlation {agreement}");
}

#[test]
fn grain_stays_on_the_canvas_when_the_source_does_not_cover_it() {
    // A grey square is stored at its own bounds, but its grain is what the full-canvas grey has.
    let square = json!({ "id": 1, "type": "shape", "params": {
        "position": [37, 21], "width": 90, "height": 70, "fill_paint": [128, 128, 128, 255] } });
    let params = json!({ "amount": 0.2, "size": 2 });
    let covering = render_scene(grained(CANVAS, grey(), params.clone()), 1.0);
    let bounded = render_scene(grained(CANVAS, square, params), 1.0);
    for y in 23..89 {
        for x in 39..125 {
            assert_eq!(bounded.pixel(x, y), covering.pixel(x, y), "({x}, {y})");
        }
    }
    assert_eq!(
        bounded.pixel(10, 10)[3],
        0,
        "nothing is added outside the square"
    );
}
