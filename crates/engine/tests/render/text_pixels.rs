//! Per-glyph transforms, fades, paint, retained styles and distance-field coverage of text, checked
//! on pixels. Each table row says which glyph render mode it expects the engine to choose.

use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::LazyLock;

use lumen_engine::composition::Composition;
use lumen_engine::gpu::{BoundFrame, GpuCompositionRenderer};
use serde_json::{Value, json};

use crate::support::{
    Case, Expect, Frame, Gpu, Scene, TestMedia, check, gpu, panic_message, render,
    render_inspecting, scene,
};

const INTER: &[u8] = include_bytes!("../../../text/assets/inter/InterVariable-Latin.ttf");
const COLR: &[u8] = include_bytes!("../../../text/assets/roboto/RobotoColrTest.ttf");

pub fn media() -> TestMedia {
    static MEDIA: LazyLock<TestMedia> = LazyLock::new(|| {
        TestMedia::default()
            .font("Inter", "text-inter", INTER.to_vec())
            .font("Roboto COLR Test", "text-colr", COLR.to_vec())
    });
    MEDIA.clone()
}

/// A one-node scene of text with `params`, over `base` params that `params` overrides.
pub fn text_scene(canvas: (u32, u32), base: Value, params: Value) -> Scene {
    scene(
        canvas,
        json!([{ "id": 1, "type": "text", "params": merged(base, params) }]),
    )
}

pub fn merged(mut base: Value, params: Value) -> Value {
    for (key, value) in params.as_object().expect("params are an object") {
        base[key] = value.clone();
    }
    base
}

pub fn frame_of(gpu: &Gpu, scene: Scene, media: &TestMedia) -> Frame {
    render(&mut gpu.renderer(), &scene.into(), 0.0, media)
}

/// Runs `run` on every row, then panics listing the rows that failed. `LUMEN_CASE=substring`
/// selects rows by name.
pub fn check_rows<R>(rows: Vec<(String, R)>, run: impl Fn(&Gpu, &R)) {
    let filter = std::env::var("LUMEN_CASE").ok();
    let gpu = gpu();
    let rows: Vec<_> = rows
        .iter()
        .filter(|(name, _)| filter.as_ref().is_none_or(|filter| name.contains(filter)))
        .collect();
    let failures: Vec<_> = rows
        .iter()
        .filter_map(|(name, row)| {
            let error = catch_unwind(AssertUnwindSafe(|| run(&gpu, row))).err()?;
            Some(format!("  {name}: {}", panic_message(&*error)))
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

/// How the engine draws the glyphs of a row.
#[derive(Clone, Copy, Debug)]
pub enum Mode {
    Raster,
    /// Distance fields.
    Msdf,
    /// Colour glyphs, which have no outline for a distance field.
    Color,
}

impl Mode {
    /// The `render_mode` param that asks for this mode.
    pub fn name(self) -> &'static str {
        match self {
            Self::Raster | Self::Color => "raster",
            Self::Msdf => "msdf",
        }
    }

    fn instance_mode(self) -> u32 {
        match self {
            Self::Raster => lumen_text::GLYPH_MODE_RASTER_MASK,
            Self::Msdf => lumen_text::GLYPH_MODE_MSDF,
            Self::Color => lumen_text::GLYPH_MODE_RASTER_COLOR,
        }
    }
}

/// The glyph render mode of every glyph the text node 1 draws in a bound frame.
fn glyph_modes(renderer: &GpuCompositionRenderer, bound: &BoundFrame) -> Vec<u32> {
    let instances = renderer
        .compiled()
        .unwrap()
        .plan
        .buffers()
        .iter()
        .find(|buffer| buffer.label.as_deref() == Some("text:1:instances"))
        .expect("a text node 1")
        .id;
    let size = std::mem::size_of::<lumen_text::GpuGlyphInstance>();
    bound
        .frame_update()
        .uploads()
        .iter()
        .filter_map(|upload| match upload {
            lumen_gpu::Upload::Buffer { id, data, .. } if *id == instances => Some(data),
            _ => None,
        })
        .flat_map(|data| data.chunks_exact(size))
        .map(bytemuck::pod_read_unaligned::<lumen_text::GpuGlyphInstance>)
        .map(|instance| instance.render_mode())
        .filter(|&mode| mode != lumen_text::GLYPH_MODE_RECT)
        .collect()
}

/// Renders `scene` on `renderer` and passes `check` the render mode of every glyph it draws.
pub fn frame_checked(
    renderer: &mut GpuCompositionRenderer,
    scene: Scene,
    media: &TestMedia,
    check: impl FnOnce(&[u32]),
) -> Frame {
    render_inspecting(renderer, &scene.into(), 0.0, media, |renderer, bound| {
        check(&glyph_modes(renderer, bound));
    })
}

/// Renders `scene` on `renderer`, whose glyphs must all draw in `mode`.
pub fn frame_with(
    renderer: &mut GpuCompositionRenderer,
    scene: Scene,
    media: &TestMedia,
    mode: Mode,
) -> Frame {
    frame_checked(renderer, scene, media, |modes| {
        let expected = mode.instance_mode();
        assert!(
            !modes.is_empty() && modes.iter().all(|&actual| actual == expected),
            "glyph modes {modes:?}, expected all {mode:?} ({expected})"
        );
    })
}

/// Renders `scene` on a new renderer, whose glyphs must all draw in `mode`.
pub fn frame_in(gpu: &Gpu, scene: Scene, media: &TestMedia, mode: Mode) -> Frame {
    frame_with(&mut gpu.renderer(), scene, media, mode)
}

pub fn alpha(frame: &Frame, x: usize, y: usize) -> u8 {
    frame.rgba[(y * frame.width as usize + x) * 4 + 3]
}

/// The total alpha, as a measure of how much ink there is.
pub fn mass(frame: &Frame) -> u64 {
    frame
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| u64::from(p[3]))
        .sum()
}

fn runs(width: usize, inked: impl Fn(usize) -> bool) -> Vec<Range<usize>> {
    let mut runs: Vec<Range<usize>> = Vec::new();
    for at in (0..width).filter(|&at| inked(at)) {
        match runs.last_mut() {
            Some(run) if run.end == at => run.end = at + 1,
            _ => runs.push(at..at + 1),
        }
    }
    runs
}

/// Runs of columns that hold any ink, left to right.
pub fn ink_columns(frame: &Frame) -> Vec<Range<usize>> {
    let (width, height) = (frame.width as usize, frame.height as usize);
    runs(width, |x| (0..height).any(|y| alpha(frame, x, y) > 0))
}

/// Runs of rows that hold any ink, top to bottom.
pub fn ink_rows(frame: &Frame) -> Vec<Range<usize>> {
    let (width, height) = (frame.width as usize, frame.height as usize);
    runs(height, |y| (0..width).any(|x| alpha(frame, x, y) > 0))
}

/// `[min_x, min_y, max_x, max_y]` of the pixels in `columns` with alpha over half, which must
/// not be empty.
pub fn ink_box(frame: &Frame, columns: Range<usize>) -> [usize; 4] {
    ink_box_in(frame, columns, 0..frame.height as usize)
}

pub fn ink_box_in(frame: &Frame, columns: Range<usize>, rows: Range<usize>) -> [usize; 4] {
    let mut bounds = [usize::MAX, usize::MAX, 0, 0];
    for (y, x) in rows.flat_map(|y| columns.clone().map(move |x| (y, x))) {
        if alpha(frame, x, y) > 128 {
            bounds = [
                bounds[0].min(x),
                bounds[1].min(y),
                bounds[2].max(x),
                bounds[3].max(y),
            ];
        }
    }
    assert!(bounds[0] != usize::MAX, "no ink in columns {columns:?}");
    bounds
}

pub fn whole(frame: &Frame) -> Range<usize> {
    0..frame.width as usize
}

/// How many pixels differ between `a` and `b`, and by how much in a channel at most.
pub fn changes(a: &Frame, b: &Frame) -> (usize, u8) {
    a.rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.rgba.as_chunks::<4>().0.iter())
        .map(|(a, b)| a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)).max().unwrap())
        .filter(|&change| change > 0)
        .fold((0, 0), |(count, most), change| {
            (count + 1, most.max(change))
        })
}

/// The intersection over union of the alpha of two frames.
pub fn iou(a: &Frame, b: &Frame) -> f64 {
    let (intersection, union) = a
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.rgba.as_chunks::<4>().0.iter())
        .fold((0u64, 0u64), |(i, u), (a, b)| {
            (i + u64::from(a[3].min(b[3])), u + u64::from(a[3].max(b[3])))
        });
    intersection as f64 / union as f64
}

/// Four "I" glyphs that stay apart, at 40 px.
const BARS: &str = "IIII";

fn bars() -> Value {
    json!({ "content": BARS, "font_family": "Inter", "font_size": 40, "position": [20, 10] })
}

/// Two accents that land on each other, in one cluster of two glyph instances.
const DOUBLE_ACCENT: &str = "e\u{301}\u{301}";

const MODES: [(&str, Mode); 2] = [("raster", Mode::Raster), ("msdf", Mode::Msdf)];

/// A group of glyphs by where its ink lies, and how the glyph params should move and fade it.
struct Move {
    name: &'static str,
    canvas: (u32, u32),
    base: fn() -> Value,
    params: Value,
    by_rows: bool,
    /// The group's shift in pixels and opacity, by its index.
    expect: fn(usize) -> (i32, i32, f32),
}

#[test]
fn glyph_transform() {
    let spans = || {
        merged(
            bars(),
            json!({ "content": [{ "text": "II" }, { "text": "II" }], "font_size": 32 }),
        )
    };
    let lines = || merged(bars(), json!({ "content": "I\nI\nI" }));
    let moves = [
        Move {
            name: "glyph index moves and fades each glyph",
            canvas: (128, 96),
            base: bars,
            params: json!({
                "glyph_offset": "=vec2(0, glyph * 10)", "glyph_opacity": "=glyph / (glyph_count - 1)"
            }),
            by_rows: false,
            expect: |group| (0, 10 * group as i32, group as f32 / 3.0),
        },
        Move {
            name: "line index moves lines and hides the last",
            canvas: (96, 160),
            base: lines,
            params: json!({
                "glyph_offset": "=vec2(line * 30, 0)", "glyph_opacity": "=line < line_count - 1"
            }),
            by_rows: true,
            expect: |group| (30 * group as i32, 0, if group < 2 { 1.0 } else { 0.0 }),
        },
        Move {
            name: "span index moves the second span",
            canvas: (128, 112),
            base: spans,
            params: json!({ "glyph_offset": "=vec2(0, 40 * span / (span_count - 1))" }),
            by_rows: false,
            expect: |group| (0, if group < 2 { 0 } else { 40 }, 1.0),
        },
        Move {
            name: "a function of the glyph index",
            canvas: (128, 96),
            base: bars,
            params: json!({ "glyph_offset": "=slide(8)" }),
            by_rows: false,
            expect: |group| (8 * group as i32, 0, 1.0),
        },
    ];
    let rows = MODES
        .into_iter()
        .flat_map(|mode| moves.iter().map(move |row| (mode, row)))
        .map(|((mode_name, mode), row)| (format!("{} ({mode_name})", row.name), (mode, row)))
        .collect();
    check_rows(rows, |gpu, &(mode, row)| {
        let render = |glyph: Value| {
            let mut scene = text_scene(
                row.canvas,
                (row.base)(),
                merged(glyph, json!({ "render_mode": mode.name() })),
            );
            scene.0["functions"] = json!({ "slide(distance)": "vec2(distance * glyph, 0)" });
            frame_in(gpu, scene, &media(), mode)
        };
        let (plain, moved) = (render(json!({})), render(row.params.clone()));
        let groups = if row.by_rows {
            ink_rows(&plain)
        } else {
            ink_columns(&plain)
        };
        assert!(groups.len() >= 3, "{groups:?}");
        let (width, height) = (plain.width as i32, plain.height as i32);
        let mut expected = vec![0.0f32; (width * height) as usize];
        for (group, band) in groups.iter().enumerate() {
            let (dx, dy, opacity) = (row.expect)(group);
            for y in 0..height {
                for x in 0..width {
                    let inside = if row.by_rows {
                        band.contains(&(y as usize))
                    } else {
                        band.contains(&(x as usize))
                    };
                    let (tx, ty) = (x + dx, y + dy);
                    if inside && (0..width).contains(&tx) && (0..height).contains(&ty) {
                        expected[(ty * width + tx) as usize] =
                            f32::from(alpha(&plain, x as usize, y as usize)) * opacity;
                    }
                }
            }
        }
        assert!(mass(&plain) > 0 && mass(&moved) > 0, "blank render");
        for y in 0..height as usize {
            for x in 0..width as usize {
                let (want, got) = (
                    expected[y * width as usize + x],
                    f32::from(alpha(&moved, x, y)),
                );
                assert!(
                    (got - want).abs() <= 1.5,
                    "({x}, {y}): alpha {got}, expected {want}"
                );
            }
        }
    });
}

#[test]
fn glyph_scale_pivots_about_each_glyph() {
    let spaced = || merged(bars(), json!({ "letter_spacing": 14 }));
    let lines = || merged(bars(), json!({ "content": "I\nI", "line_height": 80 }));
    // (name, content, scale, canvas, scaled axis is y)
    let rows = [
        (
            "wider",
            spaced as fn() -> Value,
            "=vec2(2, 1)",
            (128, 64),
            false,
        ),
        ("taller", lines, "=vec2(1, 2)", (64, 160), true),
    ];
    let rows = MODES
        .into_iter()
        .flat_map(|mode| rows.into_iter().map(move |row| (mode, row)))
        .map(|((mode_name, mode), row)| (format!("{} ({mode_name})", row.0), (mode, row)))
        .collect();
    check_rows(rows, |gpu, &(mode, (_, base, scale, canvas, vertical))| {
        let render = |params: Value| {
            let params = merged(params, json!({ "render_mode": mode.name() }));
            frame_in(gpu, text_scene(canvas, base(), params), &media(), mode)
        };
        let (plain, scaled) = (render(json!({})), render(json!({ "glyph_scale": scale })));
        let bands = |frame: &Frame| {
            if vertical {
                ink_rows(frame)
            } else {
                ink_columns(frame)
            }
        };
        let (plain_bands, scaled_bands) = (bands(&plain), bands(&scaled));
        assert_eq!(plain_bands.len(), scaled_bands.len(), "{scaled_bands:?}");
        // The extent along the scaled axis, and the middle of it, of the ink in `band`.
        let measure = |frame: &Frame, band: &Range<usize>| {
            let (columns, rows) = if vertical {
                (whole(frame), band.clone())
            } else {
                (band.clone(), 0..frame.height as usize)
            };
            let [x0, y0, x1, y1] = ink_box_in(frame, columns, rows);
            if vertical {
                (y1 - y0 + 1, y0 + y1, x1 - x0 + 1)
            } else {
                (x1 - x0 + 1, x0 + x1, y1 - y0 + 1)
            }
        };
        let mut shifts = Vec::new();
        for (before, after) in plain_bands.iter().zip(&scaled_bands) {
            let (extent, middle, across) = measure(&plain, before);
            let (scaled_extent, scaled_middle, scaled_across) = measure(&scaled, after);
            // Each edge is within a pixel of where scaling puts it.
            assert!(
                scaled_extent.abs_diff(2 * extent) <= 2,
                "extent {extent} became {scaled_extent}"
            );
            assert!(
                across.abs_diff(scaled_across) <= 1,
                "cross extent {across} became {scaled_across}"
            );
            shifts.push((scaled_middle as i32 - middle as i32) / 2);
        }
        let (least, most) = (shifts.iter().min().unwrap(), shifts.iter().max().unwrap());
        // Scaling about the node's origin would move each glyph by its distance from it.
        assert!(most - least <= 1, "the glyphs' middles moved by {shifts:?}");
        assert!(most.abs() <= 6, "the glyphs' middles moved by {shifts:?}");
    });
}

#[test]
fn glyph_rotation_turns_each_word_about_its_glyphs() {
    let rows = MODES
        .into_iter()
        .map(|(name, mode)| (name.to_string(), mode))
        .collect();
    check_rows(rows, |gpu, &mode| {
        let base = json!({ "content": "II   II", "font_family": "Inter", "font_size": 40, "position": [20, 10] });
        let render = |params: Value| {
            let params = merged(params, json!({ "render_mode": mode.name() }));
            frame_in(
                gpu,
                text_scene((160, 64), base.clone(), params),
                &media(),
                mode,
            )
        };
        let (still, turned) = (
            render(json!({})),
            render(json!({ "glyph_rotation": "=word * 90" })),
        );
        let bars = ink_columns(&still);
        assert_eq!(bars.len(), 4, "{bars:?}");
        // The first word is untouched.
        for y in 0..64 {
            for x in bars[0].start..bars[1].end {
                assert_eq!(alpha(&turned, x, y), alpha(&still, x, y), "({x}, {y})");
            }
        }
        // Each glyph of the second turns a quarter about its own middle, so the upright bars
        // lie down where they stood, beside each other.
        let word = bars[1].end + 4..160;
        let [x0, y0, x1, y1] = ink_box(&still, word.clone());
        assert!(y1 - y0 > x1 - x0, "still {:?}", [x0, y0, x1, y1]);
        let [tx0, ty0, tx1, ty1] = ink_box(&turned, word);
        assert!(
            tx1 - tx0 > 4 * (ty1 - ty0),
            "turned {:?}",
            [tx0, ty0, tx1, ty1]
        );
        assert!(
            (tx0 + tx1).abs_diff(x0 + x1) <= 4,
            "the word's middle moved"
        );
    });
}

#[test]
fn glyph_params_match_static_text() {
    let offset = |position: [u32; 2], glyph_offset: Value| {
        text_scene(
            (128, 128),
            json!({ "content": "Lumen fi", "font_size": 40, "position": position }),
            json!({ "glyph_offset": glyph_offset }),
        )
    };
    check(vec![
        Case::new(
            "static text is drawn",
            offset([32, 17], json!([0, 0])),
            Expect::Nonblank,
        ),
        Case::new(
            "a literal glyph offset moves the text",
            offset([20, 10], json!([12, 7])),
            Expect::Matches(offset([32, 17], json!([0, 0])).into()),
        ),
        Case::new(
            "a constant expression is its literal",
            text_scene(
                (128, 128),
                json!({ "content": "Lumen fi", "font_size": 40, "position": [20, 10] }),
                json!({ "glyph_offset": "=vec2(12, 7 + 0 * frame)", "glyph_scale": "=vec2(1, 1)" }),
            ),
            Expect::Matches(offset([20, 10], json!([12, 7])).into()),
        )
        .tolerance(0),
        Case::new(
            "a glyph moved 60 px lands where the text placed there does",
            text_scene(
                (128, 128),
                json!({ "content": "Hi", "font_size": 30, "position": [10, 10] }),
                json!({ "glyph_offset": [0, 60] }),
            ),
            Expect::Matches(
                text_scene(
                    (128, 128),
                    json!({ "content": "Hi", "font_size": 30, "position": [10, 70] }),
                    json!({}),
                )
                .into(),
            ),
        )
        .tolerance(0),
    ]);
}

/// The most a faded channel may differ from half the opaque one: the opaque channel was rounded
/// once, to at most half a level off, which halves, and the faded one is rounded once more.
const HALF_ROUNDING: f32 = 1.0;

struct Fade {
    name: &'static str,
    canvas: (u32, u32),
    node: Value,
    field: &'static str,
    /// The render mode of the row's glyphs, if it draws text.
    mode: Option<Mode>,
}

#[test]
fn fades_as_one_layer() {
    let red = json!([255, 0, 0, 255]);
    let gradient = json!({
        "type": "linear_gradient", "stops": [[0, [255, 0, 0, 255]], [1, [0, 0, 255, 128]]]
    });
    // Opaque on the left half of the glyphs and clear on the right, switching inside a pixel.
    let sharp = json!({
        "type": "linear_gradient", "start": [0, 0], "end": [1, 0],
        "stops": [[0, [255, 255, 255, 255]], [0.5, [255, 255, 255, 255]],
                  [0.5, [255, 255, 255, 0]], [1, [255, 255, 255, 0]]]
    });
    let text = |params: Value, mode: Mode| {
        let mode_name = mode.name();
        let base = json!({ "font_size": 40, "position": [20, 10], "render_mode": mode_name });
        json!({ "id": 1, "type": "text", "params": merged(base, params) })
    };
    let fade = |name, canvas, node: Value, field, mode| Fade {
        name,
        canvas,
        node,
        field,
        mode,
    };
    let mut fades = vec![
        fade(
            "shape",
            (56, 56),
            json!({ "id": 1, "type": "shape", "params": {
                "geometry_kind": "ellipse", "width": 40, "height": 30, "position": [4.5, 6],
                "fill_paint": gradient
            } }),
            "opacity",
            None,
        ),
        fade(
            "path",
            (56, 56),
            json!({ "id": 1, "type": "path", "params": {
                "data": "M 5 5 Q 40 0 45 40 L 5 45 Z", "fill_paint": gradient,
                "stroke_enabled": true, "stroke_width": 2
            } }),
            "opacity",
            None,
        ),
        fade(
            "text layer over overlapping marks",
            (128, 72),
            text(
                json!({ "content": "e\u{301}\u{301} f\u{301}\u{301}", "font_size": 48, "position": [8, 8] }),
                Mode::Raster,
            ),
            "opacity",
            Some(Mode::Raster),
        ),
    ];
    for (mode_name, mode) in MODES {
        fades.push(fade(
            Box::leak(format!("cluster ({mode_name})").into_boxed_str()),
            (128, 72),
            text(json!({ "content": DOUBLE_ACCENT }), mode),
            "glyph_opacity",
            Some(mode),
        ));
        fades.push(fade(
            Box::leak(format!("cluster split by wrapping ({mode_name})").into_boxed_str()),
            (128, 72),
            text(json!({ "content": "A\u{338}", "max_width": 15, "glyph_offset": "=vec2(line * 40, 0)" }), mode),
            "glyph_opacity",
            Some(mode),
        ));
        for align in ["outside", "center"] {
            fades.push(fade(
                Box::leak(format!("stroke {align} ({mode_name})").into_boxed_str()),
                (128, 112),
                text(
                    json!({
                        "content": "I", "font_size": 64, "position": [40, 20],
                        "stroke": { "paint": red, "width": 4, "align": align }
                    }),
                    mode,
                ),
                "glyph_opacity",
                Some(mode),
            ));
        }
    }
    for (name, content, stroke) in [
        ("sharp paint with a stroke", "I", true),
        ("sharp paint over a cluster", DOUBLE_ACCENT, false),
        ("sharp paint over a stroked cluster", DOUBLE_ACCENT, true),
    ] {
        let mut params = json!({
            "content": content, "font_size": 64, "position": [40.8, 20.3], "color": sharp
        });
        if stroke {
            params["stroke"] = json!({ "paint": red, "width": 4 });
        }
        fades.push(fade(
            name,
            (128, 112),
            text(params, Mode::Raster),
            "glyph_opacity",
            Some(Mode::Raster),
        ));
    }
    let rows = fades
        .into_iter()
        .map(|fade| (fade.name.to_string(), fade))
        .collect();
    check_rows(rows, |gpu, fade| {
        let render = |opacity: f64| {
            let mut node = fade.node.clone();
            node["params"][fade.field] = json!(opacity);
            let scene = scene(fade.canvas, json!([node]));
            match fade.mode {
                Some(mode) => frame_in(gpu, scene, &media(), mode),
                None => frame_of(gpu, scene, &media()),
            }
        };
        let (opaque, faded) = (render(1.0), render(0.5));
        assert!(
            opaque
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[3] >= 200),
            "the opaque render has no solid pixel"
        );
        for (index, (full, half)) in opaque.rgba.iter().zip(&faded.rgba).enumerate() {
            assert!(
                (f32::from(*full) * 0.5 - f32::from(*half)).abs() <= HALF_ROUNDING,
                "byte {index} (pixel {}, channel {}): {full} at half opacity gave {half}",
                index / 4,
                index % 4
            );
        }
    });
}

/// Text params in Inter at 32 px, with `params` added.
pub fn inter_text(content: Value, params: Value) -> Value {
    merged(
        json!({ "content": content, "font_family": "Inter", "font_size": 32, "position": [8, 20] }),
        params,
    )
}

/// Sums of red, green and blue over the mostly opaque pixels of the columns in `run`.
fn channel_sums(frame: &Frame, run: &Range<usize>) -> [u64; 3] {
    let mut sums = [0; 3];
    for y in 0..frame.height {
        for x in run.clone() {
            let pixel = frame.pixel(x as u32, y);
            if pixel[3] > 200 {
                for (sum, &channel) in sums.iter_mut().zip(&pixel) {
                    *sum += u64::from(channel);
                }
            }
        }
    }
    sums
}

#[test]
fn text_paint() {
    let transparent_stop = json!({
        "type": "linear_gradient", "units": "user_space", "start": [-1e6, 0], "end": [1e6, 0],
        "stops": [[0, [255, 0, 0, 0]], [1, [0, 0, 255, 255]]]
    });
    // A hard red-to-blue stop through the crossbar of an `H` scaled 8x, at canvas x 57.6.
    let hard_stop = json!({
        "type": "linear_gradient", "units": "user_space", "start": [0, 0], "end": [28.4, 0],
        "stops": [[0, [255, 0, 0, 255]], [0.5, [255, 0, 0, 255]], [0.5, [0, 0, 255, 255]], [1, [0, 0, 255, 255]]]
    });
    let span_gradient = json!({
        "type": "linear_gradient", "start": [0, 0], "end": [1, 0],
        "stops": [[0, [255, 0, 0, 255]], [1, [0, 0, 255, 255]]]
    });
    let mut rows = Vec::new();
    for (mode_name, mode) in MODES {
        for supersample in [false, true] {
            rows.push((
                format!(
                    "a transparent stop is premultiplied ({mode_name}, supersample {supersample})"
                ),
                (mode, supersample),
            ));
        }
    }
    check_rows(rows, |gpu, &(mode, supersample)| {
        let params = json!({
            "content": "M", "font_size": 90, "position": [0, 0], "color": transparent_stop,
            "render_mode": mode.name(), "paint_supersample": supersample
        });
        let frame = frame_in(
            gpu,
            text_scene((96, 112), json!({}), params),
            &media(),
            mode,
        );
        // Halfway along the gradient is blue at half alpha, with no red leaked from the clear stop.
        let solid: Vec<_> = frame
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| pixel[3] >= 126)
            .collect();
        assert!(!solid.is_empty(), "no pixel is half covered");
        for pixel in solid {
            assert!(
                pixel[0] <= 1
                    && pixel[1] <= 1
                    && pixel[2].abs_diff(128) <= 2
                    && pixel[3].abs_diff(128) <= 2,
                "{pixel:?}"
            );
        }
    });
    check_rows(
        vec![
            (
                "supersampling mixes the stop's pixel".to_string(),
                (true, 1..=2),
            ),
            (
                "without supersampling the stop falls between pixels".to_string(),
                (false, 0..=0),
            ),
        ],
        |gpu, (supersample, mixed_columns)| {
            let params = json!({
                "content": "H", "font_size": 16, "render_mode": "msdf", "position": [8, 8],
                "scale": [8, 8], "paint_supersample": supersample, "color": hard_stop
            });
            let frame = frame_in(
                gpu,
                text_scene((128, 128), json!({}), params),
                &media(),
                Mode::Msdf,
            );
            let pixels: Vec<_> = frame.rgba.as_chunks::<4>().0.iter().collect();
            assert!(
                pixels.iter().any(|pixel| **pixel == [255, 0, 0, 255]),
                "no red"
            );
            assert!(
                pixels.iter().any(|pixel| **pixel == [0, 0, 255, 255]),
                "no blue"
            );
            let mixed = (0..128)
                .filter(|&x| {
                    (0..128).any(|y| {
                        let pixel = frame.pixel(x, y);
                        pixel[3] == 255 && pixel[0] > 8 && pixel[2] > 8
                    })
                })
                .count();
            assert!(
                mixed_columns.contains(&mixed),
                "the stop is blurred over {mixed} columns"
            );
        },
    );
    let spans = json!([{ "text": "IIII" }, { "text": "IIII", "color": span_gradient }]);
    let rows = ["span", "text"]
        .into_iter()
        .map(|space| (format!("paint space {space}"), space))
        .collect();
    check_rows(rows, |gpu, &space| {
        let params = inter_text(
            spans.clone(),
            json!({ "position": [8, 40], "paint_space": space }),
        );
        let frame = frame_of(gpu, text_scene((128, 80), params, json!({})), &media());
        let runs = ink_columns(&frame);
        assert_eq!(runs.len(), 8, "{runs:?}");
        let red_share = |run| {
            let [red, _, blue] = channel_sums(&frame, run);
            red as f64 / (red + blue) as f64
        };
        // The first span takes the node's white.
        for run in &runs[..4] {
            let [red, green, blue] = channel_sums(&frame, run);
            assert!(green > 0 && green == red && green == blue, "{run:?}");
        }
        let (first, last) = (red_share(&runs[4]), red_share(&runs[7]));
        match space {
            "span" => assert!(first > 0.75, "{first}"),
            _ => assert!((0.3..0.7).contains(&first), "{first}"),
        }
        assert!(last < 0.25, "{last}");
    });
    // The same text and gradient however its content is written.
    let lumen = |content: Value| {
        let color = json!({ "type": "linear_gradient", "stops": [[0, [255, 0, 0, 255]], [1, [0, 0, 255, 255]]] });
        text_scene(
            (128, 64),
            inter_text(content, json!({ "position": [8, 20], "color": color })),
            json!({}),
        )
    };
    let mut cases = vec![Case::new(
        "a plain string",
        lumen(json!("Lumen")),
        Expect::Nonblank,
    )];
    for (name, content) in [
        ("a span", json!([{ "text": "Lumen" }])),
        ("an expression", json!("=\"Lumen\"")),
        ("an expression in a span", json!([{ "text": "=\"Lumen\"" }])),
    ] {
        cases.push(
            Case::new(
                name,
                lumen(content),
                Expect::Matches(lumen(json!("Lumen")).into()),
            )
            .tolerance(0),
        );
    }
    check(cases.into_iter().map(|case| case.media(media())).collect());
}

/// A field of text that an expression animates, between its value `from` at frame 0 and `to` at
/// frame 1, in text that `with` builds around the field's value.
struct Animated {
    name: &'static str,
    from: f64,
    to: f64,
    with: fn(Value) -> Value,
}

#[test]
fn retained_style() {
    let node = |style: Value| {
        text_scene(
            (160, 112),
            merged(bars(), json!({ "content": "Hi\nHo", "font_size": 40 })),
            style,
        )
    };
    let rows = [
        Animated {
            name: "span font size",
            from: 24.0,
            to: 48.0,
            with: |size| json!({ "content": [{ "text": "I" }, { "text": "I", "font_size": size }], "font_size": 24 }),
        },
        Animated {
            name: "font weight",
            from: 400.0,
            to: 500.0,
            with: |weight| json!({ "font_weight": weight }),
        },
        Animated {
            name: "letter spacing",
            from: 0.0,
            to: 20.0,
            with: |gap| json!({ "letter_spacing": gap }),
        },
        Animated {
            name: "line height",
            from: 48.0,
            to: 80.0,
            with: |height| json!({ "line_height": height }),
        },
    ];
    let rows = rows
        .into_iter()
        .map(|row| (row.name.to_string(), row))
        .collect();
    check_rows(rows, |gpu, row| {
        let animated = (row.with)(json!(format!(
            "={} + frame * {}",
            row.from,
            row.to - row.from
        )));
        let media = media();
        let mut renderer = gpu.renderer();
        let composition = Composition::from(node(animated).frames(2));
        // One renderer and its compiled plan draw the frames in turn, going back to the first.
        let mut compiled = Vec::new();
        let drawn = [0.0, 1.0, 0.0].map(|at| {
            let frame = render(&mut renderer, &composition, at, &media);
            compiled.push(renderer.plan_compile_count());
            frame
        });
        assert!(
            compiled.iter().all(|&count| count == compiled[0]),
            "later frames recompiled the plan: compile counts {compiled:?}"
        );
        let [from, to] = [row.from, row.to].map(|value| {
            let literal = frame_of(gpu, node((row.with)(json!(value))), &media);
            assert!(mass(&literal) > 0, "blank control for {value}");
            literal
        });
        for (frame, literal, value) in [
            (&drawn[0], &from, row.from),
            (&drawn[1], &to, row.to),
            (&drawn[2], &from, row.from),
        ] {
            let (changed, most) = changes(frame, literal);
            assert!(
                changed == 0,
                "frame {} differs from the literal {value} in {changed} pixels, by up to {most}",
                frame.at
            );
        }
        assert!(
            changes(&from, &to).0 > 0,
            "{} makes no difference to the render",
            row.name
        );
    });
}

fn tester(content: &str, size: f64, family: &str, style: &str, weight: u32) -> Value {
    json!({
        "content": content, "font_family": family, "font_style": style, "font_weight": weight,
        "font_size": size, "position": [12.25, 8.5]
    })
}

#[test]
fn msdf_coverage_matches_raster() {
    // (name, text params, canvas, the render mode of the distance-field render, smallest IoU)
    let rows = vec![
        (
            "Roboto at 24",
            tester("AVt8", 24.0, "Roboto", "normal", 400),
            (128, 48),
            Mode::Msdf,
            0.68,
        ),
        (
            "Roboto at 144",
            tester("AVt8", 144.0, "Roboto", "normal", 400),
            (384, 192),
            Mode::Msdf,
            0.90,
        ),
        (
            "Roboto italic at 72",
            tester("AVt8", 72.0, "Roboto", "italic", 400),
            (192, 96),
            Mode::Msdf,
            0.90,
        ),
        (
            "Inter weight 250 at 72",
            tester("Ho8a", 72.0, "Inter", "normal", 250),
            (192, 96),
            Mode::Msdf,
            0.90,
        ),
        (
            "Inter weight 850 at 72",
            tester("Ho8a", 72.0, "Inter", "normal", 850),
            (192, 96),
            Mode::Msdf,
            0.90,
        ),
        // Overlapping contours cannot be distance fields.
        (
            "Inter overlapping glyphs stay raster",
            tester("AVt", 72.0, "Inter", "normal", 400),
            (192, 96),
            Mode::Raster,
            0.99,
        ),
        (
            "colour glyphs stay raster",
            json!({ "content": "A", "font_family": "Roboto COLR Test", "font_size": 48, "position": [8, 4] }),
            (64, 64),
            Mode::Color,
            0.99,
        ),
    ];
    let rows = rows
        .into_iter()
        .map(|row| (row.0.to_string(), row))
        .collect();
    check_rows(rows, |gpu, (_, params, canvas, mode, minimum)| {
        let render = |render_mode: &str, mode: Mode| {
            let params = merged(params.clone(), json!({ "render_mode": render_mode }));
            frame_in(gpu, text_scene(*canvas, json!({}), params), &media(), mode)
        };
        let raster = render(
            "raster",
            if matches!(mode, Mode::Color) {
                Mode::Color
            } else {
                Mode::Raster
            },
        );
        let msdf = render("msdf", *mode);
        let overlap = iou(&raster, &msdf);
        assert!(
            mass(&raster) > 0 && overlap > *minimum,
            "coverage IoU {overlap:.3}"
        );
    });
}

#[test]
fn msdf_corners_stay_within_a_pixel_of_raster_at_1000px() {
    let render = |mode: Mode| {
        let params = json!({
            "content": "R", "font_size": 1000, "position": [20, -180],
            "render_mode": mode.name()
        });
        (mode, text_scene((560, 420), json!({}), params))
    };
    let gpu = gpu();
    let raster = frame_in(&gpu, render(Mode::Raster).1, &media(), Mode::Raster);
    let msdf = frame_in(&gpu, render(Mode::Msdf).1, &media(), Mode::Msdf);
    let width = raster.width as usize;
    let solid = raster
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| pixel[3] == 255)
        .count();
    assert!(solid > 20_000, "only {solid} pixels are solid");
    // Fields with too few texels per em cut facets of several pixels into sharp corners, so the
    // field must stay inside the raster outline dilated and eroded by one pixel.
    let (mut outliers, raster_alpha) = (0, |x, y| alpha(&raster, x, y));
    for y in 1..raster.height as usize - 1 {
        for x in 1..width - 1 {
            let neighbours =
                (y - 1..=y + 1).flat_map(|ny| (x - 1..=x + 1).map(move |nx| raster_alpha(nx, ny)));
            let (low, high) =
                neighbours.fold((255u8, 0u8), |(low, high), a| (low.min(a), high.max(a)));
            let value = alpha(&msdf, x, y);
            outliers += usize::from(low.saturating_sub(value).max(value.saturating_sub(high)) > 26);
        }
    }
    assert!(outliers <= 16, "{outliers} pixels off the raster outline");
}

#[test]
fn glyphs_larger_than_the_atlas_are_drawn_as_distance_fields() {
    let gpu = gpu();
    // At 6000 px the stem of an "I" is taller than the 4096 px atlas.
    let params = json!({ "content": "I", "font_size": 6000, "position": [-700, -2000] });
    let scene = text_scene((128, 128), json!({}), params);
    let frame = frame_checked(&mut gpu.renderer(), scene, &media(), |modes| {
        assert_eq!(modes, vec![lumen_text::GLYPH_MODE_MSDF]);
    });
    let solid = frame
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| pixel[3] == 255)
        .count();
    assert!(solid > 128 * 128 / 2, "{solid} solid pixels");
}

#[test]
fn msdf_transform_keeps_edges_sharp_and_turns_about_the_anchor() {
    let render = |font_size: f64, scale: f64, rotation: f64, anchor: [f64; 2]| {
        let params = json!({
            "content": "Lumen AV", "font_size": font_size, "render_mode": "msdf",
            "position": [20, 30], "scale": [scale, scale], "rotation": rotation, "anchor": anchor
        });
        let gpu = gpu();
        frame_in(
            &gpu,
            text_scene((256, 256), json!({}), params),
            &media(),
            Mode::Msdf,
        )
    };
    let edges = |frame: &Frame| {
        frame
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[3] > 0 && p[3] < 255)
            .count()
    };
    // Scaled glyphs keep distance-field edges as sharp as glyphs laid out at the larger size.
    let (scaled, larger) = (
        render(16.0, 3.0, 0.0, [0.0, 0.0]),
        render(48.0, 1.0, 0.0, [0.0, 0.0]),
    );
    assert!(iou(&scaled, &larger) > 0.9, "IoU {}", iou(&scaled, &larger));
    assert!(
        edges(&scaled).abs_diff(edges(&larger)) * 4 < edges(&larger),
        "{} edges against {}",
        edges(&scaled),
        edges(&larger)
    );
    // A quarter turn about a whole-pixel anchor moves pixel centres onto pixel centres.
    let (upright, turned) = (
        render(24.0, 2.0, 0.0, [40.0, 20.0]),
        render(24.0, 2.0, 90.0, [40.0, 20.0]),
    );
    assert!(
        upright
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[3] == 255),
        "no solid pixel"
    );
    let (pivot_x, pivot_y, size) = (60i32, 50i32, 256i32);
    let mut expected = turned.rgba.clone();
    expected.fill(0);
    for y in 0..size {
        for x in 0..size {
            let (tx, ty) = (pivot_x - (y - pivot_y) - 1, pivot_y + (x - pivot_x));
            if (0..size).contains(&tx) && (0..size).contains(&ty) {
                expected[((ty * size + tx) * 4 + 3) as usize] =
                    alpha(&upright, x as usize, y as usize);
            }
        }
    }
    let expected = Frame {
        at: 0.0,
        width: 256,
        height: 256,
        rgba: expected,
    };
    let rotated = iou(&turned, &expected);
    assert!(rotated > 0.97, "rotation IoU {rotated}");
}

#[test]
fn spans_shape_in_their_own_size_and_weight() {
    let gpu = gpu();
    let draw = |content: Value, style: Value| {
        let params = merged(
            json!({ "content": content, "font_family": "Inter", "position": [20, 20] }),
            style,
        );
        frame_of(&gpu, text_scene((128, 80), json!({}), params), &media())
    };
    let (small, large) = (
        json!({ "font_size": 24 }),
        json!({ "font_size": 48, "font_weight": 800 }),
    );
    let spans = [small.clone(), large.clone()].map(|style| merged(style, json!({ "text": "I" })));
    let pixels = draw(json!(spans), json!({}));
    let runs = ink_columns(&pixels);
    assert_eq!(runs.len(), 2, "{runs:?}");
    let inks = [0, 1].map(|index| ink_box(&pixels, runs[index].clone()));
    assert!(
        inks[0][3].abs_diff(inks[1][3]) <= 1,
        "baselines differ: {inks:?}"
    );
    // Each span's glyph is as wide and tall as the plain text in its style draws it.
    for (style, ink) in [small, large].into_iter().zip(inks) {
        let alone = draw(json!("I"), style.clone());
        let alone = ink_box(&alone, ink_columns(&alone)[0].clone());
        let extent = |[x0, y0, x1, y1]: [usize; 4]| [x1 - x0, y1 - y0];
        let ([width, height], [alone_width, alone_height]) = (extent(ink), extent(alone));
        assert!(
            width.abs_diff(alone_width) <= 1 && height.abs_diff(alone_height) <= 1,
            "{style}: {ink:?} drawn alone is {alone:?}"
        );
    }
}
