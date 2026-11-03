//! Motion blur against the average of the same layer held still at each tap of its shutter. The
//! shared table form renders one scene, so these rows are run by hand: each renders the blurred
//! scene, the held scene, and one scene per tap on a single renderer.
//!
//! A shape, path or text node samples its position, scale, rotation and anchor over the shutter,
//! and a transform node samples its scale, translation, rotation and pivot. Every other param
//! keeps its value at the frame, so a tap scene writes only those params at the tap's time.

use std::panic::{AssertUnwindSafe, catch_unwind};

use serde_json::{Value, json};

use crate::support::{Frame, Gpu, Scene, TestMedia, gpu, panic_message, render, scene};

/// Which render of a layer a row's `layer` function describes.
#[derive(Clone, Copy)]
enum Moment {
    /// Animated by expressions, with a shutter of this many degrees.
    Blurred(f64),
    /// Animated by the same expressions, without motion blur.
    Held,
    /// Held still at the values the animation has at this frame time.
    Tap(f64),
}

impl Moment {
    fn angle(self) -> f64 {
        match self {
            Self::Blurred(angle) => angle,
            Self::Held | Self::Tap(_) => 0.0,
        }
    }

    /// A param animated by `formula` of `frame`, or its value `at` a tap's frame time.
    fn param(self, formula: &str, at: impl Fn(f64) -> Value) -> Value {
        match self {
            Self::Tap(frame) => at(frame),
            Self::Blurred(_) | Self::Held => json!(format!("={formula}")),
        }
    }

    /// A position at `y` whose x is `x0` plus `speed` pixels a frame.
    fn drift(self, x0: f64, speed: f64, y: f64) -> Value {
        self.param(&format!("vec2({x0} + frame * {speed}, {y})"), |t| {
            json!([x0 + t * speed, y])
        })
    }
}

type Layer = fn(Moment) -> Scene;

enum Check {
    /// The blurred render is the average of this many taps.
    Taps(usize),
    /// The blurred render is the blurred render of this layer.
    Like(Layer),
    /// The layer does not move, so blurring it renders exactly what holding it does.
    Held,
}

struct Row {
    name: &'static str,
    layer: Layer,
    frame: f64,
    angle: f64,
    check: Check,
    tolerance: u8,
    media: TestMedia,
    ink: Vec<[u32; 4]>,
}

/// A row at `frame` with a shutter of `angle` degrees, averaging `taps` taps. Production takes a
/// tap per pixel that the layer travels, up to 32, so `taps` is the count it would take.
/// `tolerance` is how far in any channel the blurred render may be from the average, which
/// allows for the rounding of each 8-bit tap and for how each kind of layer draws its edges.
fn row(
    name: &'static str,
    layer: Layer,
    frame: f64,
    angle: f64,
    taps: usize,
    tolerance: u8,
) -> Row {
    Row {
        name,
        layer,
        frame,
        angle,
        check: Check::Taps(taps),
        tolerance,
        media: TestMedia::default(),
        ink: Vec::new(),
    }
}

impl Row {
    fn like(self, other: Layer) -> Self {
        Self {
            check: Check::Like(other),
            ..self
        }
    }

    fn held(name: &'static str, layer: Layer) -> Self {
        Self {
            check: Check::Held,
            ..row(name, layer, 3.0, 180.0, 0, 0)
        }
    }

    fn media(self, media: TestMedia) -> Self {
        Self { media, ..self }
    }

    /// Rectangles `[x0, y0, x1, y1]` where both the blurred render and its expectation have ink.
    fn ink(self, ink: &[[u32; 4]]) -> Self {
        Self {
            ink: ink.to_vec(),
            ..self
        }
    }

    fn run(&self, gpu: &Gpu) {
        let mut renderer = gpu.renderer();
        let mut draw = |layer: Layer, moment| {
            render(
                &mut renderer,
                &layer(moment).into(),
                self.frame,
                &self.media,
            )
        };
        let blurred = draw(self.layer, Moment::Blurred(self.angle));
        let held = draw(self.layer, Moment::Held);
        let expected = match self.check {
            Check::Held => {
                assert!(alpha(&held) > 0, "the held layer is blank");
                assert!(
                    blurred.rgba == held.rgba,
                    "blurring changed a layer that does not move"
                );
                return;
            }
            Check::Like(other) => draw(other, Moment::Blurred(self.angle)),
            Check::Taps(taps) => average(
                &tap_frames(self.frame, self.angle, taps)
                    .into_iter()
                    .map(|frame| draw(self.layer, Moment::Tap(frame)))
                    .collect::<Vec<_>>(),
            ),
        };
        assert!(alpha(&expected) > 0, "the expected render is blank");
        let (worst, at) = worst_difference(&blurred, &expected);
        assert!(
            worst <= self.tolerance,
            "blurred differs from its expected render by {worst} at {at:?}, allowed {}",
            self.tolerance
        );
        let mean = mean_difference(&blurred, &expected);
        assert!(
            mean <= MEAN_TOLERANCE,
            "blurred differs from its expected render by {mean:.3} on average"
        );
        let (moved, at) = worst_difference(&blurred, &held);
        assert!(
            moved > 16,
            "blurred is within {moved} of the held layer, near {at:?}"
        );
        for &[x0, y0, x1, y1] in &self.ink {
            let ink = |frame: &Frame| {
                (y0..y1)
                    .flat_map(|y| (x0..x1).map(move |x| (x, y)))
                    .map(|(x, y)| u32::from(frame.pixel(x, y)[3]))
                    .sum::<u32>()
            };
            assert!(
                ink(&blurred) > 0 && ink(&expected) > 0,
                "no ink in {:?}: {} blurred, {} expected",
                [x0, y0, x1, y1],
                ink(&blurred),
                ink(&expected)
            );
        }
    }
}

/// Frames at the middle of each of `taps` equal parts of a shutter of `angle` degrees centred on
/// `frame`.
fn tap_frames(frame: f64, angle: f64, taps: usize) -> Vec<f64> {
    (0..taps)
        .map(|tap| {
            let s = (2 * tap + 1) as f64 / taps as f64 - 1.0;
            frame + s * angle / 720.0
        })
        .collect()
}

fn average(frames: &[Frame]) -> Frame {
    let first = &frames[0];
    let rgba = (0..first.rgba.len())
        .map(|index| {
            let sum: f64 = frames.iter().map(|f| f64::from(f.rgba[index])).sum();
            (sum / frames.len() as f64).round() as u8
        })
        .collect();
    Frame {
        at: first.at,
        width: first.width,
        height: first.height,
        rgba,
    }
}

fn alpha(frame: &Frame) -> u64 {
    frame
        .rgba
        .iter()
        .skip(3)
        .step_by(4)
        .map(|&a| u64::from(a))
        .sum()
}

/// The largest channel difference between two frames, and the pixel it is at.
fn worst_difference(a: &Frame, b: &Frame) -> (u8, (u32, u32)) {
    let mut worst = (0, 0);
    for (index, (a, b)) in a.rgba.iter().zip(&b.rgba).enumerate() {
        if a.abs_diff(*b) > worst.0 {
            worst = (a.abs_diff(*b), index as u32 / 4);
        }
    }
    (worst.0, (worst.1 % a.width, worst.1 / a.width))
}

/// Bounds the average difference of a row whose largest allowed difference is wide.
const MEAN_TOLERANCE: f64 = 0.5;

fn mean_difference(a: &Frame, b: &Frame) -> f64 {
    let sum: u64 = a
        .rgba
        .iter()
        .zip(&b.rgba)
        .map(|(a, b)| u64::from(a.abs_diff(*b)))
        .sum();
    sum as f64 / a.rgba.len() as f64
}

fn check(rows: Vec<Row>) {
    let filter = std::env::var("LUMEN_CASE").ok();
    let gpu = gpu();
    let rows: Vec<_> = rows
        .iter()
        .filter(|row| filter.as_deref().is_none_or(|f| row.name.contains(f)))
        .collect();
    let failures: Vec<_> = rows
        .iter()
        .filter_map(|row| {
            let error = catch_unwind(AssertUnwindSafe(|| row.run(&gpu))).err()?;
            Some(format!("  {}: {}", row.name, panic_message(&*error)))
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {} rows failed:\n{}",
        failures.len(),
        rows.len(),
        failures.join("\n")
    );
}

const ORANGE: [u8; 4] = [255, 128, 0, 255];
const PLATE: &str = "plate";

fn plate() -> TestMedia {
    TestMedia::default().image(PLATE, 16, 16, vec![255; 16 * 16 * 4])
}

/// A square canvas of `nodes`, long enough to render at frame 3.
fn canvas(size: u32, nodes: Value) -> Scene {
    scene((size, size), nodes).frames(4)
}

fn shape(m: Moment, params: Value) -> Value {
    let mut params = params;
    params["fill_paint"] = json!(ORANGE);
    params["motion_blur"] = json!(m.angle());
    json!({ "id": 1, "type": "shape", "params": params })
}

fn path(m: Moment, data: &str, position: Value, scale: Value) -> Value {
    json!({ "id": 1, "type": "path", "params": {
        "data": data, "position": position, "scale": scale, "fill_paint": ORANGE,
        "motion_blur": m.angle(),
    } })
}

fn text(m: Moment, content: &str, size: u32, params: Value) -> Value {
    let mut params = params;
    params["content"] = json!(content);
    params["font_size"] = json!(size);
    params["motion_blur"] = json!(m.angle());
    json!({ "id": 1, "type": "text", "params": params })
}

/// The plate through a `transform` whose x translation is `x0` plus `speed` a frame.
fn moved_plate(m: Moment, x0: f64, speed: f64, rotate: f64) -> Scene {
    let x = m.param(&format!("{x0} + frame * {speed}"), |t| {
        json!(x0 + t * speed)
    });
    canvas(
        64,
        json!([
            { "id": 1, "type": "media_in", "params": { "kind": "image", "source": PLATE } },
            { "id": 2, "type": "transform", "in": { "source": 1 }, "params": {
                "translate_x": x, "translate_y": 20, "rotate": rotate, "motion_blur": m.angle(),
            } },
        ]),
    )
}

fn rotated_shape(m: Moment) -> Scene {
    let params = json!({
        "width": 20, "height": 12, "rotation": 15, "anchor": [10, 6],
        "position": m.drift(10.0, 31.0, 30.0),
    });
    canvas(96, json!([shape(m, params)]))
}

/// An 8 pixel square 32 pixels above the canvas centre turns about it by 90 degrees a frame.
/// Pixels on the square's arc, at 55, 90 and 125 degrees from straight up.
const ON_ARC: &[[u32; 4]] = &[[90, 45, 91, 46], [96, 64, 97, 65], [90, 82, 91, 83]];

fn orbiting_square(m: Moment) -> Scene {
    let params = json!({
        "width": 8, "height": 8, "position": [60, 28], "anchor": [4, 36],
        "rotation": m.param("frame * 90", |t| json!(t * 90.0)),
    });
    canvas(128, json!([shape(m, params)]))
}

fn plate_moving(m: Moment) -> Scene {
    moved_plate(m, 10.0, 19.0, 10.0)
}

/// A 40 × 20 rectangle, a path round it, a curve or text, whose x scale crosses zero at frame 1
/// and is not zero at any other time of a 360 degree shutter.
fn scale_through_zero(m: Moment, kind: usize) -> Scene {
    let scale = m.param("vec2(frame - 1, 1)", |t| json!([t - 1.0, 1.0]));
    let at = json!([20, 30]);
    let rectangle = "M 0 0 L 40 0 L 40 20 L 0 20 Z";
    let word = json!({ "render_mode": "msdf", "position": [10, 30], "scale": scale });
    let layer = match kind {
        0 => shape(
            m,
            json!({ "width": 40, "height": 20, "position": at, "scale": scale }),
        ),
        1 => path(m, rectangle, at, scale),
        2 => path(m, "M 0 0 C 40 0 40 40 0 40 Z", at, scale),
        _ => text(m, "Lumen", 32, word),
    };
    canvas(96, json!([layer]))
}

/// Scale stretching along x at the shutter's open and along y at its close, or the reverse.
fn stretching(m: Moment, reverse: bool) -> Scene {
    let (open, close) = if reverse {
        ("frame - 1", "1 - frame")
    } else {
        ("1 - frame", "frame - 1")
    };
    let formula = format!("vec2(0.5 + 5 * max(0, {open}), 0.5 + 5 * max(0, {close}))");
    let scale = m.param(&formula, |t| {
        let [x, y] = [(1.0 - t).max(0.0), (t - 1.0).max(0.0)].map(|s| 0.5 + 5.0 * s);
        if reverse {
            json!([y, x])
        } else {
            json!([x, y])
        }
    });
    let params = json!({ "render_mode": "msdf", "position": [10, 10], "scale": scale });
    canvas(192, json!([text(m, "Hi", 32, params)]))
}

fn slide(m: Moment, content: &str, mode: &str, x0: f64, speed: f64) -> Scene {
    let params = json!({ "render_mode": mode, "position": m.drift(x0, speed, 40.0) });
    canvas(192, json!([text(m, content, 32, params)]))
}

/// Combining marks overlap their base glyphs, and the paint and the layer are translucent, so
/// summing glyphs instead of compositing them would count the overlaps twice.
fn marks(m: Moment, mode: &str) -> Scene {
    let params = json!({
        "render_mode": mode, "opacity": 0.6, "color": [255, 255, 255, 128],
        "position": m.drift(20.0, 2.999, 30.0),
    });
    canvas(128, json!([text(m, "A\u{338} f\u{301}", 48, params)]))
}

/// A hard gradient stop falls within the enlarged glyph, and paint supersampling antialiases it.
fn supersampled_gradient(m: Moment) -> Scene {
    let params = json!({
        "render_mode": "msdf", "scale": [2, 2], "paint_supersample": true,
        "color": { "type": "linear_gradient", "start": [0.5, 0], "end": [0.52, 0], "stops": [
            [0, "#ff0000"], [0.5, "#ff0000"], [0.5, "#0000ff"], [1, "#0000ff"]
        ] },
        "position": m.drift(20.0, 2.999, 30.0),
    });
    canvas(128, json!([text(m, "H", 40, params)]))
}

/// The second glyph lies wholly below the text's box, and is smeared by the text's motion.
fn glyphs_beyond_box(m: Moment) -> Scene {
    let params = json!({
        "position": m.drift(10.0, 60.0, 10.0),
        "glyph_offset": "=vec2(0, glyph * 180)", "glyph_rotation": "=glyph * 30",
    });
    canvas(256, json!([text(m, "II", 40, params)]))
}

/// Half transparent red through `unpremultiply`, a transform, and `premultiply`, or through the
/// transform alone.
fn translucent(m: Moment, sampling: &str, x0: f64, speed: f64, straight: bool) -> Scene {
    let convert = |id: u32, source: u32, mode: &str| {
        json!({ "id": id, "type": "alpha_premultiply", "in": { "source": source },
            "params": { "mode": mode } })
    };
    let background = json!({ "id": 1, "type": "background", "params": {
        "paint": [255, 0, 0, 128], "width": 8, "height": 4
    } });
    let x = m.param(&format!("{x0} + frame * {speed}"), |t| {
        json!(x0 + t * speed)
    });
    let transform = |source: u32| {
        json!({ "id": 3, "type": "transform", "in": { "source": source }, "params": {
            "translate_x": x, "sampling": sampling, "motion_blur": m.angle(),
        } })
    };
    let nodes = if straight {
        let unpremultiply = convert(2, 1, "unpremultiply");
        json!([
            background,
            unpremultiply,
            transform(2),
            convert(4, 3, "premultiply")
        ])
    } else {
        json!([background, transform(1)])
    };
    canvas(64, nodes)
}

fn raster_marks(m: Moment) -> Scene {
    marks(m, "raster")
}

fn msdf_marks(m: Moment) -> Scene {
    marks(m, "msdf")
}

/// At frame 0 the text lies wholly beyond the left edge.
fn entering(m: Moment) -> Scene {
    slide(m, "Lumen", "raster", -150.0, 400.0)
}

/// The text is far wider than the canvas, so the taps show a region wider than it.
fn scrolling(m: Moment) -> Scene {
    slide(m, "MMMMMMMMMMMM", "raster", -60.0, 64.0)
}

fn raster_smears(m: Moment) -> Scene {
    slide(m, "Lumen", "raster", -20.0, 40.0)
}

fn msdf_smears(m: Moment) -> Scene {
    slide(m, "Lumen", "msdf", -20.0, 40.0)
}

fn opening(m: Moment) -> Scene {
    stretching(m, false)
}

fn closing(m: Moment) -> Scene {
    stretching(m, true)
}

fn zero_shape(m: Moment) -> Scene {
    scale_through_zero(m, 0)
}

fn zero_straight(m: Moment) -> Scene {
    scale_through_zero(m, 1)
}

fn zero_curved(m: Moment) -> Scene {
    scale_through_zero(m, 2)
}

fn zero_text(m: Moment) -> Scene {
    scale_through_zero(m, 3)
}

fn straight_nearest(m: Moment) -> Scene {
    translucent(m, "nearest", 0.0, 20.0, true)
}

fn straight_linear(m: Moment) -> Scene {
    translucent(m, "linear", 0.5, 2.999, true)
}

fn premultiplied_linear(m: Moment) -> Scene {
    translucent(m, "linear", 0.5, 2.999, false)
}

fn still_shape(m: Moment) -> Scene {
    let params = json!({
        "width": 30, "height": 20, "position": "=vec2(30.5, 40)", "rotation": 20,
        "stroke_enabled": true,
    });
    canvas(64, json!([shape(m, params)]))
}

fn still_path(m: Moment) -> Scene {
    let (position, scale) = (json!("=vec2(20, 10 + 0 * frame)"), json!([1.5, 0.75]));
    canvas(
        64,
        json!([path(m, "M 5 5 Q 40 0 45 40 L 5 45 Z", position, scale)]),
    )
}

fn still_text(m: Moment) -> Scene {
    let params = json!({ "render_mode": "msdf", "position": "=vec2(10, 30)", "rotation": 10 });
    canvas(64, json!([text(m, "Lumen", 24, params)]))
}

fn still_plate(m: Moment) -> Scene {
    moved_plate(m, 20.0, 0.0, 30.0)
}

// Each tap is an 8-bit render, and text edges differ from the average of the taps by a few more
// levels. Glyphs at the largest scale of the shutter are drawn at its resolution, and each tap
// at its own.
#[test]
fn motion_matches_taps() {
    check(vec![
        row("rotated shape", rotated_shape, 2.0, 180.0, 16, 2),
        row("arc", orbiting_square, 1.0, 360.0, 32, 2).ink(ON_ARC),
        row("image", plate_moving, 1.0, 180.0, 10, 2).media(plate()),
        row("raster marks", raster_marks, 0.0, 360.0, 3, 2),
        row("msdf marks", msdf_marks, 0.0, 360.0, 3, 2),
        row("supersampled", supersampled_gradient, 0.0, 360.0, 3, 2),
        row("entry from off-canvas", entering, 0.0, 360.0, 32, 4),
        row("wide scroll", scrolling, 0.0, 360.0, 32, 8).ink(&[[191, 0, 192, 192]]),
        row("open and close", opening, 1.0, 360.0, 32, 24),
        row("close and open", closing, 1.0, 360.0, 0, 2).like(opening),
        row("zero shape", zero_shape, 1.0, 360.0, 32, 2),
        row("zero straight path", zero_straight, 1.0, 360.0, 32, 2),
        row("zero curved path", zero_curved, 1.0, 360.0, 32, 2),
        row("zero text", zero_text, 1.0, 360.0, 32, 16),
        row("glyphs beyond box", glyphs_beyond_box, 1.0, 180.0, 30, 4).ink(&[[0, 200, 256, 256]]),
        row("raster smears cross", raster_smears, 2.0, 360.0, 32, 6),
        row("msdf smears cross", msdf_smears, 2.0, 360.0, 32, 6),
        row("straight nearest", straight_nearest, 1.0, 360.0, 20, 2),
        // One tap filtered alone weights straight colour by coverage differently from the blur,
        // so the straight chain is compared with the chain that has no conversions.
        row("straight linear", straight_linear, 0.0, 360.0, 0, 2).like(premultiplied_linear),
    ]);
}

#[test]
fn still_layers_are_unchanged() {
    // An expression that holds still is evaluated over the shutter like a moving one.
    check(vec![
        Row::held("shape", still_shape),
        Row::held("path", still_path),
        Row::held("text", still_text),
        Row::held("image", still_plate).media(plate()),
    ]);
}
