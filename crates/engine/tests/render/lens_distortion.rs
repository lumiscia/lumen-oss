//! `lens_distortion` on known shapes: a grid for the round trip, small squares whose shift has a
//! closed form, and a white square whose colour fringes sit at known edges.

use lumen_engine::composition::Composition;
use serde_json::{Value, json};

use crate::support::{
    BLACK, CLEAR, Case, Expect, Frame, Scene, TestMedia, WHITE, check, gpu, render, scene,
};

const SIZE: u32 = 128;
/// Half the canvas diagonal in pixels, the unit of the lens's distances.
const HALF_DIAGONAL: f64 = SIZE as f64 * std::f64::consts::SQRT_2 / 2.0;

fn rect(id: u32, position: [u32; 2], size: [u32; 2], paint: [u8; 4]) -> Value {
    json!({ "id": id, "type": "shape", "params": {
        "position": position, "width": size[0], "height": size[1], "fill_paint": paint } })
}

fn background(paint: [u8; 4]) -> Value {
    json!({ "id": 1, "type": "background", "params": { "paint": paint } })
}

/// `layers`, the first of them the base, merged one over the next into the last node.
fn stacked(layers: Vec<Value>) -> Vec<Value> {
    let mut nodes = layers.clone();
    let mut merged = layers[0]["id"].clone();
    for (index, layer) in layers.iter().enumerate().skip(1) {
        let id = 200 + index;
        nodes.push(json!({ "id": id, "type": "merge",
            "in": { "base": merged, "overlay": layer["id"] } }));
        merged = json!(id);
    }
    nodes
}

/// Black lines 8 pixels wide every 32 pixels on white, blurred so that resampling it twice
/// changes it little.
fn grid() -> Vec<Value> {
    let mut layers = vec![background(WHITE)];
    for line in 0..4 {
        let at = 12 + 32 * line;
        layers.push(rect(10 + line, [at, 0], [8, SIZE], BLACK));
        layers.push(rect(20 + line, [0, at], [SIZE, 8], BLACK));
    }
    let mut nodes = stacked(layers);
    let merged = nodes.last().expect("merged layers")["id"].clone();
    nodes.push(json!({ "id": 99, "type": "blur", "params": { "radius": 3 },
        "in": { "source": merged } }));
    nodes
}

/// `nodes`, then a `lens_distortion` of the last of them with each of `lenses` after it.
fn lensed(mut nodes: Vec<Value>, lenses: &[Value]) -> Scene {
    let mut from = nodes.last().expect("a source")["id"].clone();
    for (index, params) in lenses.iter().enumerate() {
        let id = 100 + index;
        nodes.push(
            json!({ "id": id, "type": "lens_distortion", "params": params,
            "in": { "source": from } }),
        );
        from = json!(id);
    }
    scene((SIZE, SIZE), Value::Array(nodes))
}

fn render_scene(scene: Scene) -> Frame {
    let composition: Composition = scene.into();
    render(
        &mut gpu().renderer(),
        &composition,
        0.0,
        &TestMedia::default(),
    )
}

/// The mean difference per colour byte over pixels at most `radius` half-diagonals from the middle.
fn mean_difference(a: &Frame, b: &Frame, radius: f64) -> f64 {
    let (mut total, mut count) = (0.0, 0.0);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let offset = (f64::from(x) - 63.5).hypot(f64::from(y) - 63.5) / HALF_DIAGONAL;
            if offset <= radius {
                let (a, b) = (a.pixel(x, y), b.pixel(x, y));
                total += (0..3).map(|c| f64::from(a[c].abs_diff(b[c]))).sum::<f64>();
                count += 3.0;
            }
        }
    }
    total / count
}

#[test]
fn zero_params_pass_the_source_through() {
    let source = || scene((SIZE, SIZE), Value::Array(grid()));
    check(vec![Case::new(
        "no distortion",
        lensed(grid(), &[json!({})]),
        Expect::Matches(source().into()),
    )]);
}

#[test]
fn undistort_inverts_distort() {
    let distort = json!({ "k1": 0.2, "k2": 0.05 });
    let undistort = json!({ "k1": 0.2, "k2": 0.05, "direction": "undistort" });
    let original = render_scene(lensed(grid(), &[]));
    let distorted = render_scene(lensed(grid(), std::slice::from_ref(&distort)));
    let round_trip = render_scene(lensed(grid(), &[distort, undistort]));
    let moved = mean_difference(&original, &distorted, 0.7);
    let restored = mean_difference(&original, &round_trip, 0.7);
    assert!(moved > 20.0, "distorting changed the grid by only {moved}");
    assert!(
        restored < moved / 8.0,
        "undistorting left {restored} of the {moved} the distortion moved"
    );
}

/// The centroid of the red in `frame` within the pixels `x` and `y` span.
fn centroid(frame: &Frame, x: std::ops::Range<u32>, y: std::ops::Range<u32>) -> [f64; 2] {
    let (mut sum, mut weight) = ([0.0; 2], 0.0);
    for row in y {
        for column in x.clone() {
            let w = f64::from(frame.pixel(column, row)[0]) / 255.0;
            sum[0] += w * (f64::from(column) + 0.5);
            sum[1] += w * (f64::from(row) + 0.5);
            weight += w;
        }
    }
    assert!(weight > 1.0, "no square in the window");
    [sum[0] / weight, sum[1] / weight]
}

/// How far from the middle a square shown at `distance` half-diagonals lies in the source: solves
/// `r (1 + k1 r²) = distance`.
fn barrel_radius(k1: f64, distance: f64) -> f64 {
    let (mut low, mut high) = (0.0, distance);
    for _ in 0..60 {
        let mid = (low + high) / 2.0;
        if mid * (1.0 + k1 * mid * mid) < distance {
            low = mid;
        } else {
            high = mid;
        }
    }
    low
}

#[test]
fn barrel_moves_corners_in_more_than_edges() {
    const K1: f64 = 0.2;
    let nodes = stacked(vec![
        background(BLACK),
        rect(2, [118, 62], [3, 3], WHITE),
        rect(3, [118, 118], [3, 3], WHITE),
    ]);
    let frame = render_scene(lensed(nodes, &[json!({ "k1": K1 })]));
    let shift = |window: [std::ops::Range<u32>; 2], from: [f64; 2]| {
        let [x, y] = window;
        let found = centroid(&frame, x, y);
        let radius = |at: [f64; 2]| (at[0] - 64.0).hypot(at[1] - 64.0);
        let expected =
            radius(from) - barrel_radius(K1, radius(from) / HALF_DIAGONAL) * HALF_DIAGONAL;
        (radius(from) - radius(found), expected)
    };
    let (edge, edge_expected) = shift([100..128, 40..88], [119.5, 63.5]);
    let (corner, corner_expected) = shift([100..128, 100..128], [119.5, 119.5]);
    assert!(
        (edge - edge_expected).abs() < 0.4 && (corner - corner_expected).abs() < 0.4,
        "squares moved in {edge} and {corner}, expected {edge_expected} and {corner_expected}"
    );
    assert!(
        corner > edge * 1.5,
        "the corner moved {corner}, the edge {edge}"
    );
}

#[test]
fn fill_leaves_nothing_transparent() {
    let red = vec![background([255, 0, 0, 255])];
    let transparent = |fit: &str| json!({ "k1": 0.3, "edge": "transparent", "fit": fit });
    let opaque = |frames: &[Frame]| {
        for frame in frames {
            let clear = frame.rgba.chunks(4).filter(|pixel| pixel[3] != 255).count();
            assert_eq!(clear, 0, "{clear} pixels are not opaque");
        }
    };
    check(vec![
        Case::new(
            "without fill the corners read beyond the source",
            lensed(red.clone(), &[transparent("none")]),
            Expect::Probes(vec![(0, 0, CLEAR), (SIZE - 1, SIZE - 1, CLEAR)]),
        ),
        Case::new(
            "fill zooms in until every pixel reads inside",
            lensed(red.clone(), &[transparent("fill")]),
            Expect::Custom(Box::new(opaque)),
        ),
        Case::new(
            "fill undistorting reads inside too",
            lensed(
                red,
                &[
                    json!({ "k1": -0.1, "direction": "undistort", "edge": "transparent", "fit": "fill" }),
                ],
            ),
            Expect::Custom(Box::new(opaque)),
        ),
    ]);
}

#[test]
fn chromatic_aberration_puts_red_outside_blue() {
    let nodes = stacked(vec![background(BLACK), rect(2, [84, 52], [24, 24], WHITE)]);
    let frame = render_scene(lensed(nodes, &[json!({ "chromatic_aberration": 0.1 })]));
    // The square's right edge, on the far side of the middle, shows only red where red is
    // magnified past green, and its left edge only blue where blue is shrunk past it.
    assert_eq!(frame.pixel(110, 64), [255, 0, 0, 255]);
    assert_eq!(frame.pixel(83, 64), [0, 0, 255, 255]);
}
