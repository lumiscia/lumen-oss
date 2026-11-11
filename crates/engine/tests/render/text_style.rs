//! Decorations, strokes, overflow and fit of text, checked on pixels.

use std::collections::HashMap;

use lumen_engine::composition::Composition;
use lumen_engine::gpu::GpuCompositionRenderer;
use serde_json::{Value, json};

use crate::color::light;
use crate::support::{Case, Expect, Frame, Gpu, Scene, check, render, scene};
use crate::text_pixels::{
    Mode, alpha, changes, check_rows, frame_of, frame_with, ink_box, ink_columns, ink_rows,
    inter_text, mass, media, merged, text_scene, whole,
};

const WHITE: [u8; 4] = [255; 4];
const RED: [u8; 4] = [255, 0, 0, 255];

/// Inter text with `params`, on a square canvas.
fn styled(size: u32, content: Value, params: Value) -> Scene {
    text_scene((size, size), inter_text(content, json!({})), params)
}

fn draw(gpu: &Gpu, size: u32, content: Value, params: Value) -> Frame {
    frame_of(gpu, styled(size, content, params), &media())
}

/// `[min_x, min_y, max_x, max_y]` of the pixels where `a` and `b` differ.
fn diff_box(a: &Frame, b: &Frame) -> Option<[usize; 4]> {
    let mut bounds: Option<[usize; 4]> = None;
    for (y, x) in (0..a.height).flat_map(|y| (0..a.width).map(move |x| (y as usize, x as usize))) {
        if a.pixel(x as u32, y as u32) != b.pixel(x as u32, y as u32) {
            let [x0, y0, x1, y1] = bounds.get_or_insert([x, y, x, y]);
            (*x0, *y0, *x1, *y1) = ((*x0).min(x), (*y0).min(y), (*x1).max(x), (*y1).max(y));
        }
    }
    bounds
}

/// A way to draw a text, which a table row runs after being handed the device.
type Scenario = fn(&Gpu);

fn run_scenarios(rows: Vec<(&str, Scenario)>) {
    check_rows(
        rows.into_iter()
            .map(|(name, run)| (name.to_string(), run))
            .collect(),
        |gpu, run| run(gpu),
    );
}

struct Decoration {
    name: &'static str,
    /// Content and params of the render without the decoration, and with it.
    plain: (Value, Value),
    decorated: (Value, Value),
    check: fn(&Frame, &Frame, [usize; 4]),
}

#[test]
fn decorations() {
    let clear = json!({ "color": [0, 0, 0, 0] });
    let spans = |second: Value| json!([{ "text": "II" }, merged(json!({ "text": "II" }), second)]);
    let rows = vec![
        Decoration {
            name: "a span's underline lies under that span alone",
            plain: (spans(json!({})), json!({})),
            decorated: (spans(json!({ "underline": {} })), json!({})),
            check: |plain, decorated, [x0, y0, x1, y1]| {
                let runs = ink_columns(plain);
                assert_eq!(runs.len(), 4, "{runs:?}");
                let baseline = ink_box(plain, runs[0].clone())[3];
                assert!(
                    x0 + 1 >= runs[1].end && x1 <= runs[3].end + 2,
                    "{x0}..{x1} is not under {runs:?}"
                );
                assert!(y0 > baseline && y1 < baseline + 12, "{y0}..{y1}");
                assert_eq!(
                    decorated.pixel(((x0 + x1) / 2) as u32, ((y0 + y1) / 2) as u32),
                    WHITE
                );
            },
        },
        Decoration {
            name: "a highlight breaks into a padded box per line",
            plain: (json!("II II"), clear.clone()),
            decorated: (
                json!("II II"),
                merged(
                    clear.clone(),
                    json!({
                        "position": [8, 8], "max_width": 40, "line_height": 48,
                        "highlight": { "paint": RED, "padding": [4, 2], "radius": 2 }
                    }),
                ),
            ),
            check: |_, decorated, _| {
                let rows = ink_rows(decorated);
                assert_eq!(rows.len(), 2, "{rows:?}");
                let boxes = [&rows[0], &rows[1]].map(|rows| {
                    let middle = (rows.start + rows.end) / 2;
                    let inked: Vec<usize> = whole(decorated)
                        .filter(|&x| alpha(decorated, x, middle) > 128)
                        .collect();
                    [inked[0], *inked.last().unwrap(), rows.len()]
                });
                // Each line's box starts at the text's left edge less its padding.
                assert!(boxes[0][0].abs_diff(4) <= 1, "{boxes:?}");
                assert_eq!(boxes[0], boxes[1]);
                assert_eq!(
                    decorated.pixel(boxes[0][0] as u32 + 4, rows[0].start as u32 + 4),
                    RED
                );
            },
        },
        Decoration {
            name: "a strikethrough is over glyphs and a highlight under them",
            plain: (json!("IIII"), json!({ "font_size": 48 })),
            decorated: (
                json!("IIII"),
                json!({
                    "font_size": 48, "highlight": { "paint": [0, 0, 255, 255] },
                    "strikethrough": { "paint": RED, "thickness": 4 }
                }),
            ),
            check: |plain, decorated, _| {
                let runs = ink_columns(plain);
                assert_eq!(runs.len(), 4, "{runs:?}");
                let column =
                    |x: usize| (0..decorated.height).map(move |y| decorated.pixel(x as u32, y));
                let (stem, gap) = ((runs[0].start + runs[0].end) / 2, runs[0].end + 1);
                assert!(gap < runs[1].start, "{runs:?}");
                assert!(
                    column(stem).any(|pixel| pixel == RED),
                    "no strikethrough over the glyph"
                );
                assert!(
                    column(stem).any(|pixel| pixel == WHITE),
                    "no glyph over the highlight"
                );
                assert!(column(gap).any(|pixel| pixel == RED));
                assert!(column(gap).any(|pixel| pixel == [0, 0, 255, 255]));
            },
        },
        Decoration {
            name: "an extent runs on across a line break",
            plain: (json!("IIII\nIIII"), clear.clone()),
            decorated: (
                json!("IIII\nIIII"),
                merged(
                    clear.clone(),
                    json!({
                        "line_height": 48,
                        "underline": { "paint": RED, "thickness": 3, "extent": [0, 0.75] }
                    }),
                ),
            ),
            check: |_, decorated, _| {
                // 0.75 of the two lines' width is all of the first line and half of the second.
                let rows = ink_rows(decorated);
                assert_eq!(rows.len(), 2, "{rows:?}");
                let [first, second] = [&rows[0], &rows[1]].map(|rows| {
                    let y = (rows.start + rows.end) / 2;
                    whole(decorated)
                        .filter(|&x| alpha(decorated, x, y) > 128)
                        .count() as f64
                });
                assert!((second / first - 0.5).abs() < 0.05, "{second} of {first}");
            },
        },
    ];
    let rows = rows
        .into_iter()
        .map(|row| (row.name.to_string(), row))
        .collect();
    check_rows(rows, |gpu, row| {
        let render =
            |(content, params): &(Value, Value)| draw(gpu, 128, content.clone(), params.clone());
        let (plain, decorated) = (render(&row.plain), render(&row.decorated));
        let changed = diff_box(&plain, &decorated).expect("the decoration draws nothing");
        (row.check)(&plain, &decorated, changed);
    });
    let underline = |extent: Value| {
        styled(
            128,
            json!("IIII\nIIII"),
            json!({
                "color": [0, 0, 0, 0], "line_height": 48,
                "underline": { "paint": RED, "thickness": 3, "extent": extent }
            }),
        )
    };
    check(vec![
        Case::new(
            "an animated extent is the literal one",
            underline(json!("=vec2(0, frame / 4)")),
            Expect::Matches(underline(json!([0, 0.75])).into()),
        )
        .frames([3.0])
        .tolerance(0)
        .media(media()),
    ]);
}

#[test]
fn decoration_dots_stretch_with_a_nonuniform_scale() {
    // Clear glyphs leave only the underline: circles of 4 px, stretched to 16 px along x.
    let scene = styled(
        256,
        json!("IIIIII"),
        json!({
            "color": [0, 0, 0, 0], "scale": [4, 1],
            "underline": { "style": "dotted", "thickness": 4, "paint": RED }
        }),
    );
    check(vec![
        Case::new(
            "dotted underline",
            scene,
            Expect::Custom(Box::new(|frames| {
                let dots = ink_columns(&frames[0]);
                assert!(dots.len() >= 3, "{dots:?}");
                for dot in &dots[1..dots.len() - 1] {
                    assert!((14..=19).contains(&dot.len()), "{dots:?}");
                }
                let rows = ink_rows(&frames[0]);
                assert_eq!(rows.len(), 1, "{rows:?}");
                assert!((4..=6).contains(&rows[0].len()), "{rows:?}");
            })),
        )
        .media(media()),
    ]);
}

#[test]
fn decorations_split_by_span_paint_fit_the_size_counted_at_compile() {
    let spans = scene(
        (64, 32),
        json!([{ "id": 1, "type": "text", "params": {
            "content": [
                { "text": "a", "color": "#ff0000" },
                { "text": "b", "color": "#0000ff" }
            ],
            "font_size": 24, "position": [4, 2], "underline": {}
        } }]),
    );
    check(vec![Case::new(
        "an underline over two span colours",
        spans,
        Expect::Custom(Box::new(|frames| {
            let has = |color: [u8; 3]| {
                frames[0]
                    .rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| pixel[3] == 255 && pixel[..3] == color)
            };
            assert!(has([255, 0, 0]) && has([0, 0, 255]));
        })),
    )]);
}

#[test]
fn span_null_clears_what_the_node_sets() {
    let rows = ["underline", "stroke", "highlight"]
        .into_iter()
        .map(|field| (field.to_string(), field))
        .collect();
    check_rows(rows, |gpu, &field| {
        let spans = |kinds: [Value; 3]| {
            let spans: Vec<Value> = kinds
                .into_iter()
                .map(|extra| merged(json!({ "text": "II" }), extra))
                .collect();
            json!(spans)
        };
        let own = json!({ field: {} });
        let node = own.clone();
        let none = json!({});
        let cleared = draw(
            gpu,
            128,
            spans([none.clone(), json!({ field: null }), none.clone()]),
            node.clone(),
        );
        let set = draw(
            gpu,
            128,
            spans([own.clone(), none.clone(), own]),
            none.clone(),
        );
        let all = draw(gpu, 128, json!("IIIIII"), node);
        let plain = draw(gpu, 128, json!("IIIIII"), none);
        assert!(changes(&plain, &all).0 > 0, "{field} draws nothing");
        assert!(
            cleared.rgba == set.rgba,
            "a null span differs from its siblings alone setting {field}"
        );
        assert!(
            cleared.rgba != all.rgba,
            "a null span still has the node's {field}"
        );
    });
}

#[test]
fn bounded_text_matches_the_full_frame_reference() {
    let bounded = |name: &str, content: Value, params: Value| {
        Case::new(name, styled(128, content, params), Expect::Nonblank)
            .bounded(true)
            .media(media())
    };
    let stroke = json!({ "position": [32, 32], "stroke": { "paint": RED, "width": 12 } });
    check(vec![
        bounded(
            "a wavy underline",
            json!("II"),
            json!({ "underline": { "style": "wavy", "thickness": 4, "offset": 20 } }),
        ),
        bounded(
            "highlight padding",
            json!("II"),
            json!({ "highlight": { "paint": RED, "padding": [12, 16] } }),
        ),
        bounded("a raster stroke", json!("II"), stroke.clone()),
        bounded(
            "a distance-field stroke",
            json!("II"),
            merged(stroke, json!({ "render_mode": "msdf" })),
        ),
        bounded(
            "a span raised past the line box",
            json!([{ "text": "II" }, { "text": "II", "baseline_shift": 24 }]),
            json!({}),
        ),
        bounded(
            "an ellipsis",
            json!("II II\nII II\nII II"),
            json!({ "position": [8, 8], "line_height": 40, "max_lines": 2, "overflow": "ellipsis" }),
        ),
    ]);
}

#[test]
fn text_ellipsis_ends_the_last_line_shown() {
    let gpu = crate::support::gpu();
    let content = json!("II II\nII II\nII II");
    let text = |extra: Value| {
        draw(
            &gpu,
            128,
            content.clone(),
            merged(
                json!({ "position": [8, 8], "line_height": 40, "max_lines": 2 }),
                extra,
            ),
        )
    };
    // `max_lines` alone drops the lines past it.
    let clipped = text(json!({}));
    let ellipsized = text(json!({ "overflow": "ellipsis" }));
    let rows = ink_rows(&clipped);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(ink_rows(&ellipsized).len(), 2);
    // The ellipsis follows the second line.
    let second_line_end = whole(&clipped)
        .filter(|&x| rows[1].clone().any(|y| alpha(&clipped, x, y) > 0))
        .max()
        .unwrap();
    let [x0, y0, ..] = diff_box(&clipped, &ellipsized).expect("an ellipsis");
    assert!(x0 > second_line_end && y0 >= rows[1].start, "{x0}, {y0}");
    // It is the last glyph, so hiding it leaves the clipped text.
    let hidden = text(
        json!({ "overflow": "ellipsis", "glyph_opacity": "=min(1, glyph_count - 1 - glyph)" }),
    );
    assert!(hidden.rgba == clipped.rgba);
}

fn fit(extra: Value) -> Value {
    merged(
        json!({ "max_width": 64, "max_lines": 1, "fit": "shrink", "min_font_size": 4 }),
        extra,
    )
}

#[test]
fn text_fit() {
    run_scenarios(vec![
        ("shrinks into its box and measures as drawn", |gpu| {
            let params = merged(
                fit(json!({})),
                json!({ "position": "=vec2(100 - text_width(node(1)), 20)" }),
            );
            let fitted = draw(gpu, 128, json!("IIIIIIIIIIIIIIII"), params);
            let columns = ink_columns(&fitted);
            assert_eq!(columns.len(), 16, "{columns:?}");
            let [start, end] = [columns[0].start, columns[15].end];
            assert!(end - start <= 64, "{start}..{end}");
            // `text_width` measures the shrunk text, which ends where it says.
            assert!(end.abs_diff(100) <= 2, "{end}");
            assert_eq!(ink_rows(&fitted).len(), 1);
        }),
        (
            "stops shrinking at its smallest size and clips the rest",
            |gpu| {
                let params = merged(
                    fit(json!({ "min_font_size": 30 })),
                    json!({ "position": "=vec2(100 - text_width(node(1)), 20)" }),
                );
                let clipped = draw(gpu, 128, json!("IIIIIIIIIIIIIIII"), params);
                let rows = ink_rows(&clipped);
                assert_eq!(rows.len(), 1, "{rows:?}");
                // The glyphs are as tall as 30 px text, not the 14 px that would fit.
                assert!(rows[0].len() >= 20, "{rows:?}");
                let columns = ink_columns(&clipped);
                assert!(columns.len() < 16, "{columns:?}");
                assert!(
                    columns.last().unwrap().end.abs_diff(100) <= 2,
                    "{columns:?}"
                );
            },
        ),
        ("narrows the stroke with the text", |gpu| {
            let ink = |fitted: bool, stroke: bool| {
                let mut params = json!({ "font_size": 64, "position": [20, 20] });
                if fitted {
                    params = merged(params, fit(json!({ "max_width": 16 })));
                }
                if stroke {
                    params["stroke"] = json!({ "paint": RED, "width": 8 });
                }
                let frame = draw(gpu, 128, json!("II"), params);
                ink_box(&frame, whole(&frame))
            };
            let (plain, stroked) = (ink(false, false), ink(false, true));
            let (fitted, fitted_stroked) = (ink(true, false), ink(true, true));
            let scale = (fitted[3] - fitted[1]) as f32 / (plain[3] - plain[1]) as f32;
            assert!(scale < 0.6, "{scale}");
            assert!((plain[1] - stroked[1]).abs_diff(8) <= 1);
            let reach = (fitted[1] - fitted_stroked[1]) as f32;
            assert!(
                (reach - 8.0 * scale).abs() <= 1.5,
                "{reach} at scale {scale}"
            );
        }),
    ]);
}

#[test]
fn typography_params_move_ink() {
    run_scenarios(vec![
        ("a raised span is still drawn whole", |gpu| {
            let spans =
                |shift: u32| json!([{ "text": "II" }, { "text": "II", "baseline_shift": shift }]);
            let (plain, raised) = (
                draw(gpu, 128, spans(0), json!({})),
                draw(gpu, 128, spans(24), json!({})),
            );
            let (plain_runs, raised_runs) = (ink_columns(&plain), ink_columns(&raised));
            assert_eq!(raised_runs, plain_runs);
            for (run, lift) in [(0, 0), (2, 24)] {
                let top = ink_box(&plain, plain_runs[run].clone())[1];
                let raised_top = ink_box(&raised, raised_runs[run].clone())[1];
                assert_eq!(raised_top + lift, top, "run {run}");
            }
        }),
        ("uppercase draws capitals", |gpu| {
            let upper = draw(
                gpu,
                128,
                json!("ii"),
                json!({ "text_transform": "uppercase" }),
            );
            let capitals = draw(gpu, 128, json!("II"), json!({}));
            assert!(mass(&capitals) > 0 && upper.rgba == capitals.rgba);
        }),
        ("paragraph spacing separates paragraphs", |gpu| {
            let paragraphs = |spacing: u32| {
                ink_rows(&draw(
                    gpu,
                    128,
                    json!("I\nI"),
                    json!({ "paragraph_spacing": spacing }),
                ))
            };
            let (tight, spaced) = (paragraphs(0), paragraphs(10));
            assert_eq!(tight.len(), 2);
            assert_eq!(spaced[0], tight[0]);
            assert_eq!(spaced[1].start, tight[1].start + 10);
        }),
        (
            "weights grow the ink every ten units, animated or not",
            |gpu| {
                let media = media();
                for mode in [Mode::Raster, Mode::Msdf] {
                    let scene = |weight: Value| {
                        let params = json!({
                            "font_size": 96, "font_weight": weight, "render_mode": mode.name()
                        });
                        styled(160, json!("H"), params)
                    };
                    let animated = Composition::from(scene(json!("=400 + frame * 10")).frames(11));
                    let (mut literals, mut sweep) = (gpu.renderer(), gpu.renderer());
                    let mut previous = 0;
                    for step in 0..=10 {
                        let weight = 400 + step * 10;
                        let literal = frame_with(&mut literals, scene(json!(weight)), &media, mode);
                        let ink = mass(&literal);
                        assert!(
                            ink > previous,
                            "{mode:?}: weight {weight} has ink {ink}, after {previous}"
                        );
                        previous = ink;
                        let moved = render(&mut sweep, &animated, f64::from(step), &media);
                        // Atlas placement depends on the glyphs before it, and moves MSDF samples by a level.
                        let (_, most) = changes(&moved, &literal);
                        assert!(
                            most <= 1,
                            "{mode:?}: animated weight {weight} is {most} levels from the literal"
                        );
                    }
                }
            },
        ),
        ("letter spacing and line height lay text out", |gpu| {
            let media = media();
            for mode in [Mode::Raster, Mode::Msdf] {
                let mut renderer = gpu.renderer();
                let mut draw = |params: Value| {
                    let params = merged(
                        json!({ "font_size": 40, "render_mode": mode.name() }),
                        params,
                    );
                    frame_with(
                        &mut renderer,
                        styled(192, json!("Hi\nHo"), params),
                        &media,
                        mode,
                    )
                };
                let end = |frame: &Frame| {
                    let right = ink_columns(frame).last().expect("ink").end;
                    (right, ink_rows(frame).last().expect("ink").end)
                };
                let auto = draw(json!({}));
                let (right, bottom) = end(&auto);
                assert!(right > 20, "{mode:?}: nothing rendered");
                let explicit = draw(json!({ "line_height": f64::from(40.0_f32 * 1.2) }));
                assert!(
                    auto.rgba == explicit.rgba,
                    "{mode:?}: an explicit automatic line height differs"
                );
                let spaced = end(&draw(json!({ "letter_spacing": 20 })));
                assert!(
                    spaced.0 >= right + 15 && spaced.1 == bottom,
                    "{mode:?}: spaced {spaced:?}, automatic {:?}",
                    (right, bottom)
                );
                let tight = end(&draw(json!({ "letter_spacing": -4 })));
                assert!(
                    tight.0 < right && tight.1 == bottom,
                    "{mode:?}: tight {tight:?}, automatic {:?}",
                    (right, bottom)
                );
                let tall = end(&draw(json!({ "line_height": 100 })));
                assert!(
                    tall.0 == right && tall.1 >= bottom + 40,
                    "{mode:?}: tall {tall:?}, automatic {:?}",
                    (right, bottom)
                );
            }
        }),
    ]);
}

#[test]
fn stroke_rings_glyphs_outside_under_their_fill_or_centred_over_it() {
    let near = |pixel: [u8; 4], expected: [u8; 4]| {
        pixel
            .iter()
            .zip(expected)
            .all(|(&channel, expected)| channel.abs_diff(expected) <= 8)
    };
    let mut rows = Vec::new();
    for (mode_name, mode) in [("raster", Mode::Raster), ("msdf", Mode::Msdf)] {
        for (align, reach) in [("outside", 4), ("center", 2)] {
            rows.push((format!("{align} ({mode_name})"), (mode, align, reach)));
        }
    }
    check_rows(rows, |gpu, &(mode, align, reach)| {
        let base = json!({ "font_size": 64, "position": [40, 20], "render_mode": mode.name() });
        let plain = draw(gpu, 128, json!("I"), base.clone());
        let stroke = json!({ "paint": RED, "width": 4, "align": align });
        let pixels = draw(
            gpu,
            128,
            json!("I"),
            merged(base, json!({ "stroke": stroke })),
        );
        let [x0, y0, x1, y1] = ink_box(&plain, whole(&plain));
        let [sx0, sy0, sx1, sy1] = ink_box(&pixels, whole(&pixels));
        for grown in [x0 - sx0, y0 - sy0, sx1 - x1, sy1 - y1] {
            assert!(
                grown.abs_diff(reach) <= 1,
                "{:?} around {:?}, expected a reach of {reach}",
                [sx0, sy0, sx1, sy1],
                [x0, y0, x1, y1]
            );
        }
        // An outside stroke shows past the edge and the fill covers its inner half; a centred one
        // covers the glyph's edge.
        let middle = (y0 + y1) / 2;
        let edge = pixels.pixel(
            if align == "outside" { x0 - 2 } else { x0 } as u32,
            middle as u32,
        );
        assert!(near(edge, RED), "{edge:?} at the edge");
        let stem = pixels.pixel(((x0 + x1) / 2) as u32, middle as u32);
        assert!(near(stem, WHITE), "{stem:?} on the stem");
    });
}

#[test]
fn minified_distance_field_stroke_stays_off_padding_and_counters() {
    let gpu = crate::support::gpu();
    // At 48 texels per em the stroke reaches as far as its field holds, and once scaled down a
    // pixel spans more texels than the widest antialiasing.
    let params = json!({
        "font_size": 120, "position": [20, 20], "scale": [0.1, 0.1], "render_mode": "msdf"
    });
    let plain = draw(&gpu, 64, json!("O"), params.clone());
    let pixels = draw(
        &gpu,
        64,
        json!("O"),
        merged(
            params,
            json!({ "stroke": { "paint": RED, "width": 18.75 } }),
        ),
    );
    assert!(
        pixels
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[0] > 100 && pixel[1] == 0),
        "no stroke drawn"
    );
    // The glyph quad's padding past the ink's corner, and the middle of the counter, which read
    // saturated texels.
    let [x0, y0, x1, y1] = ink_box(&plain, whole(&plain));
    for (x, y) in [(x0 - 2, y0 - 2), ((x0 + x1) / 2, (y0 + y1) / 2)] {
        assert_eq!(pixels.pixel(x as u32, y as u32), [0; 4], "at {x}, {y}");
    }
}

/// A glyph drawn with strokes of different widths, to check how the stroke grows.
struct Stroke {
    name: &'static str,
    mode: Mode,
    align: &'static str,
    glyph: &'static str,
    font_size: u32,
    position: [f64; 2],
    scale: f64,
    canvas: u32,
    /// Widths at which the stroke changes the range of its distance field.
    tiers: &'static [f32],
    /// The most a channel may change across a tier, which is by rounding, if it is checked.
    allowed: Option<u8>,
    /// Whether the stroke grows from each tier's width to the next, which the scale makes
    /// visible and the canvas holds.
    ladder: bool,
    /// Pairs of widths far enough apart to show at this scale, the narrower first.
    growth: &'static [(f32, f32)],
}

impl Stroke {
    fn scene(&self, width: Option<f32>) -> Scene {
        let mut params = json!({
            "content": self.glyph, "font_family": "Inter", "font_size": self.font_size,
            "position": self.position, "scale": [self.scale, self.scale],
            "render_mode": self.mode.name()
        });
        if let Some(width) = width {
            params["stroke"] = json!({ "paint": RED, "width": width, "align": self.align });
        }
        text_scene((self.canvas, self.canvas), json!({}), params)
    }

    fn draw(&self, renderer: &mut GpuCompositionRenderer, width: Option<f32>) -> Frame {
        frame_with(renderer, self.scene(width), &media(), self.mode)
    }

    /// How far out from the glyph's edge a stroke of `width` reaches, in pixels.
    fn reach(&self, width: f32) -> f32 {
        let outside = if self.align == "outside" { 1.0 } else { 0.5 };
        width * outside * self.scale as f32
    }
}

/// A width of the stroke whose range tiers are of a 240 px glyph.
const TIERS: &[f32] = &[3.75, 18.75, 48.75, 108.75];

/// Asserts `wide` is `narrow` with its outline pushed out by `reach` pixels: no pixel is less
/// covered, and the coverage grows by at least a fraction of the ring that much reach adds to
/// a contour as long as the ink's box is wide and tall, if that is visible, so a stroke that
/// saturates at a tier or contracts does not pass.
fn assert_grows(off: &Frame, narrow: &Frame, wide: &Frame, reach: f32) {
    for y in 0..off.height as usize {
        for x in 0..off.width as usize {
            assert!(
                u16::from(alpha(wide, x, y)) + 2 >= u16::from(alpha(narrow, x, y)),
                "({x}, {y}) lost coverage: {} to {}",
                alpha(narrow, x, y),
                alpha(wide, x, y)
            );
        }
    }
    let [x0, y0, x1, y1] = ink_box(off, whole(off));
    let ring = 0.5 * reach * ((x1 - x0 + 1) + (y1 - y0 + 1)) as f32 * 255.0;
    let grown = mass(wide) as f32 - mass(narrow) as f32;
    // A quarter of a pixel's coverage is within the rounding of the pixels along the contour.
    assert!(
        ring < 64.0 || grown >= ring,
        "coverage grew by {grown}, expected at least {ring} for {reach} px"
    );
    // The widest stroke does not run off the canvas, where its growth would be hidden.
    let edge = |x: usize, y: usize| alpha(wide, x, y);
    let last = (wide.width as usize - 1, wide.height as usize - 1);
    assert!(
        (0..=last.0).all(|x| edge(x, 0) == 0 && edge(x, last.1) == 0)
            && (0..=last.1).all(|y| edge(0, y) == 0 && edge(last.0, y) == 0),
        "the widest stroke reaches the canvas edge"
    );
}

#[test]
fn stroke_continuity() {
    const ROUND: Stroke = Stroke {
        name: "",
        mode: Mode::Msdf,
        align: "outside",
        glyph: "O",
        font_size: 240,
        position: [70.3, 50.3],
        scale: 0.5,
        canvas: 256,
        tiers: TIERS,
        allowed: None,
        ladder: true,
        growth: &[],
    };
    const WIDENING: &[(f32, f32)] = &[(2.0, 4.0), (4.0, 8.0), (8.0, 16.0)];
    const I: Stroke = Stroke {
        glyph: "I",
        position: [40.0, 40.0],
        scale: 1.0,
        canvas: 128,
        tiers: &[],
        ladder: false,
        growth: WIDENING,
        ..ROUND
    };
    let rows = vec![
        Stroke {
            name: "distance-field tiers at full size",
            position: [100.3, 70.3],
            scale: 1.0,
            canvas: 448,
            allowed: Some(2),
            ladder: false,
            ..ROUND
        },
        Stroke {
            name: "distance-field tiers minified a hundred times",
            position: [32.3, 32.3],
            scale: 0.01,
            canvas: 64,
            allowed: Some(4),
            ..ROUND
        },
        Stroke {
            name: "distance-field growth through every tier",
            ..ROUND
        },
        Stroke {
            name: "a small distance-field font keeps widening",
            font_size: 24,
            ..I
        },
        Stroke {
            name: "a raster stroke keeps widening",
            mode: Mode::Raster,
            font_size: 64,
            position: [40.0, 20.0],
            ..I
        },
        Stroke {
            name: "a centred raster stroke keeps widening",
            mode: Mode::Raster,
            align: "center",
            font_size: 64,
            position: [40.0, 20.0],
            ..I
        },
    ];
    let rows = rows
        .into_iter()
        .map(|row| (row.name.to_string(), row))
        .collect();
    check_rows(rows, |gpu, row| {
        let mut renderer = gpu.renderer();
        let off = row.draw(&mut renderer, None);
        // Each width is drawn once. Crossing a tier is compared with a width 0.001 px either side.
        let mut widths: Vec<f32> = row
            .growth
            .iter()
            .flat_map(|&(narrow, wide)| [narrow, wide])
            .chain(row.tiers.iter().copied())
            .collect();
        if row.allowed.is_some() {
            widths.extend(
                row.tiers
                    .iter()
                    .flat_map(|&limit| [limit - 0.001, limit + 0.001]),
            );
        }
        let mut frames = HashMap::new();
        for width in widths {
            frames
                .entry(width.to_bits())
                .or_insert_with(|| row.draw(&mut renderer, Some(width)));
        }
        let at = |width: f32| &frames[&width.to_bits()];
        // A stroke that draws nothing would pass every comparison below.
        let widest = row
            .growth
            .last()
            .map(|&(_, wide)| wide)
            .or(row.tiers.last().copied());
        let widest = widest.expect("a row draws strokes");
        assert!(mass(&off) > 0, "blank control");
        assert!(
            changes(&off, at(widest)).0 > 0,
            "a stroke of {widest} draws nothing"
        );
        for &(narrow, wide) in row.growth {
            assert_grows(
                &off,
                at(narrow),
                at(wide),
                row.reach(wide) - row.reach(narrow),
            );
        }
        // A tier's field covers a wider range, so crossing it changes the render by no more than
        // the rounding that moving the width by 0.001 px does inside a field.
        if let Some(allowed) = row.allowed {
            for &limit in row.tiers {
                let within = changes(at(limit - 0.001), at(limit)).0;
                let (changed, most) = changes(at(limit), at(limit + 0.001));
                assert!(
                    changed <= 2 * within.max(1) && most <= allowed,
                    "width {limit}: {changed} pixels changed by up to {most}, against {within} within a field"
                );
            }
        }
        let mut previous = (0.0, &off);
        for &width in row.tiers.iter().filter(|_| row.ladder) {
            assert_grows(
                &off,
                previous.1,
                at(width),
                row.reach(width) - row.reach(previous.0),
            );
            previous = (width, at(width));
        }
    });
}

#[test]
fn hairline_strokes_fade_in_with_their_width() {
    let glyph = |mode: Mode, width: Option<f32>, size: u32, scale: f64, position: [f64; 2]| {
        let mut params = json!({
            "content": "I", "font_size": size, "position": position, "scale": [scale, scale],
            "render_mode": mode.name()
        });
        if let Some(width) = width {
            params["stroke"] = json!({ "paint": RED, "width": width, "align": "center" });
        }
        (params, mode)
    };
    let rows = vec![
        ("raster".to_string(), Mode::Raster),
        ("msdf".to_string(), Mode::Msdf),
    ];
    check_rows(rows, |gpu, &mode| {
        let draw = |width| {
            let (params, mode) = glyph(mode, width, 64, 1.0, [40.0, 20.0]);
            frame_with(
                &mut gpu.renderer(),
                text_scene((128, 128), json!({}), params),
                &media(),
                mode,
            )
        };
        let plain = draw(None);
        // A hairline at the contour draws almost nothing, and a visible stroke draws a lot.
        let most = changes(&plain, &draw(Some(0.01))).1;
        assert!(most <= 4, "a hairline changed a channel by {most}");
        let most = changes(&plain, &draw(Some(2.0))).1;
        assert!(
            most >= 100,
            "a 2 px stroke changed a channel by only {most}"
        );
        // A stroke between the smallest width kept and a quarter pixel is drawn, if faintly.
        let faint = changes(&plain, &draw(Some(0.3))).1;
        assert!(
            (1..most).contains(&faint),
            "a 0.3 px stroke changed a channel by {faint}"
        );
    });
    // Centred strokes either side of a 1/512 px half width, magnified 50 times, show and fade
    // with their width.
    let gpu = crate::support::gpu();
    let mut renderer = gpu.renderer();
    let mut draw = |width| {
        let (params, mode) = glyph(Mode::Msdf, Some(width), 2, 50.0, [48.0, 24.0]);
        frame_with(
            &mut renderer,
            text_scene((128, 128), json!({}), params),
            &media(),
            mode,
        )
    };
    let (plain, thinner, thicker, wide) = (draw(0.0), draw(0.0039062), draw(0.0039063), draw(0.2));
    let shown = changes(&plain, &thinner).1;
    let (changed, most) = changes(&thinner, &thicker);
    assert!(
        shown >= 8,
        "the thinner stroke changed a channel by {shown}"
    );
    assert!(most <= 1, "{changed} pixels changed by up to {most}");
    assert_grows(&plain, &thinner, &wide, (0.2 - 0.0039062) / 2.0 * 50.0);
}

/// Like `changes`, in premultiplied linear light on a scale of 0 to 255, where coverage adds up.
fn light_changes(a: &Frame, b: &Frame) -> (usize, f64) {
    let pixels = |frame: &Frame| -> Vec<[f64; 4]> {
        frame
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|pixel| light(*pixel))
            .collect()
    };
    pixels(a)
        .into_iter()
        .zip(pixels(b))
        .map(|(a, b)| {
            (0..4)
                .map(|channel| (a[channel] - b[channel]).abs() * 255.0)
                .fold(0.0, f64::max)
        })
        .filter(|&change| change > 0.0)
        .fold((0, 0.0), |(count, most), change| {
            (count + 1, f64::max(most, change))
        })
}

#[test]
fn distance_field_stroke_follows_its_fill_across_font_sizes() {
    // Growing small text by 0.001 px moves its stroke about as much as its fill. Software
    // rasterizers sample a few levels apart, and a stroke field coarser than the fill's would
    // change by tens.
    let rows = [12, 24, 48]
        .map(|size| (format!("font size {size}"), size))
        .into();
    check_rows(rows, |gpu, &size| {
        let mut renderer = gpu.renderer();
        let mut draw = |font_size: f64, stroke: bool| {
            let mut params = json!({
                "content": "O", "font_family": "Inter", "font_size": font_size,
                "position": [30.3, 30.3], "render_mode": "msdf"
            });
            if stroke {
                params["stroke"] = json!({ "paint": RED, "width": 1, "align": "center" });
            }
            frame_with(
                &mut renderer,
                text_scene((192, 192), json!({}), params),
                &media(),
                Mode::Msdf,
            )
        };
        let size = f64::from(size);
        let plain = draw(size, false);
        let fill = light_changes(&draw(size - 0.001, false), &plain).1;
        let stroked = draw(size, true);
        let (changed, most) = light_changes(&draw(size - 0.001, true), &stroked);
        assert!(mass(&stroked) > mass(&plain), "the stroke adds no coverage");
        assert!(
            most <= fill + 8.0,
            "{changed} pixels changed by up to {most:.1}, against {fill:.1} for the fill"
        );
    });
}

#[test]
fn a_family_no_font_answers_to_warns_once_per_node() {
    let gpu = crate::support::gpu();
    let mut renderer = gpu.renderer();
    let content = json!([{ "text": "a", "font_family": "Missing" }, { "text": "b" }]);
    let composition: Composition = styled(32, content, json!({})).frames(2).into();
    render(&mut renderer, &composition, 0.0, &media());
    let warnings = renderer.take_diagnostics();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].code, "font.fallback");
    assert_eq!(warnings[0].node, Some(lumen_engine::node::NodeId::new(1)));
    assert!(
        warnings[0].message.contains("`Missing`"),
        "{}",
        warnings[0].message
    );

    render(&mut renderer, &composition, 1.0, &media());
    assert!(renderer.take_diagnostics().is_empty());
}
