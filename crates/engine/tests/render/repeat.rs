//! Repeated shapes and paths against the same node drawn once per copy, merged in index order.

use lumen_engine::{composition::Composition, node::NodeId};
use serde_json::{Value, json};

use crate::support::{Case, Expect, Scene, TestMedia, check, gpu, render, scene};

const SIZE: (u32, u32) = (112, 64);
const CLEAR: [u8; 4] = [0, 0, 0, 0];

/// `params` with the fields of `more` added over them.
fn with(mut params: Value, more: Value) -> Value {
    params
        .as_object_mut()
        .unwrap()
        .extend(more.as_object().unwrap().clone());
    params
}

fn repeated(kind: &str, params: Value, repeat: Value) -> Scene {
    scene(
        SIZE,
        json!([{ "id": 1, "type": kind, "params": with(params, json!({ "repeat": repeat })) }]),
    )
}

/// A node of `kind` per entry of `copies`, each `params` with that entry's fields, merged in order.
fn separate(kind: &str, params: Value, copies: impl IntoIterator<Item = Value>) -> Scene {
    let mut nodes: Vec<Value> = (1..)
        .zip(copies)
        .map(|(id, more)| json!({ "id": id, "type": kind, "params": with(params.clone(), more) }))
        .collect();
    let layers: Vec<usize> = (1..=nodes.len()).collect();
    nodes.push(json!({ "id": 100, "type": "raster_multimerge", "in": { "layers": layers } }));
    scene(SIZE, json!(nodes))
}

/// A box with a red fill and a white stroke, so a copy that overlaps the one before it covers
/// part of that copy's stroke.
fn outlined_box() -> Value {
    json!({
        "width": 16, "height": 12, "position": [8, 20], "anchor": [8, 6],
        "fill_paint": [255, 0, 0, 255], "stroke_enabled": true, "stroke_width": 2,
        "stroke_paint": [255, 255, 255, 255]
    })
}

fn triangle() -> Value {
    json!({
        "data": "M 0 0 L 14 4 L 4 14 Z", "position": [6, 6], "anchor": [6, 6],
        "stroke_enabled": true, "stroke_width": 1.5
    })
}

fn copies_match_separate_nodes() -> Vec<Case> {
    let shape_case = |name: &str, repeat: Value, copies: Vec<Value>| {
        Case::new(
            name,
            repeated("shape", outlined_box(), repeat),
            Expect::Matches(separate("shape", outlined_box(), copies).into()),
        )
    };
    vec![
        shape_case(
            "a position offset steps each copy, later copies over earlier ones",
            json!({ "count": 4, "position_offset": [12, 2] }),
            (0..4)
                .map(|i| json!({ "position": [8 + 12 * i, 20 + 2 * i] }))
                .collect(),
        ),
        shape_case(
            "a rotation offset turns each copy about its own anchor",
            json!({ "count": 3, "position_offset": [30, 0], "rotation_offset": 35 }),
            (0..3)
                .map(|i| json!({ "position": [8 + 30 * i, 20], "rotation": 35 * i }))
                .collect(),
        ),
        shape_case(
            "a scale offset scales each copy about its own anchor",
            json!({ "count": 3, "position_offset": [30, 0], "scale_offset": [0.5, -0.25] }),
            (0..3)
                .map(|i| {
                    let i = f64::from(i);
                    json!({ "position": [8.0 + 30.0 * i, 20.0], "scale": [1.0 + 0.5 * i, 1.0 - 0.25 * i] })
                })
                .collect(),
        ),
        shape_case(
            "an opacity offset fades each copy, clamped at 0",
            json!({ "count": 4, "position_offset": [10, 0], "opacity_offset": -0.4 }),
            [1.0, 0.6, 0.2, 0.0]
                .into_iter()
                .zip(0..)
                .map(|(opacity, i)| json!({ "position": [8 + 10 * i, 20], "opacity": opacity }))
                .collect(),
        ),
        Case::new(
            "a path repeats in a grid with every offset",
            repeated(
                "path",
                triangle(),
                json!({
                    "count": 5, "columns": 3, "position_offset": [30, 2], "row_offset": [4, 28],
                    "rotation_offset": -20, "scale_offset": [0.2, 0.2], "opacity_offset": -0.15
                }),
            ),
            Expect::Matches(
                separate(
                    "path",
                    triangle(),
                    (0..5).map(|i| {
                        let (column, row) = (f64::from(i % 3), f64::from(i / 3));
                        let i = f64::from(i);
                        json!({
                            "position": [6.0 + 30.0 * column + 4.0 * row, 6.0 + 2.0 * column + 28.0 * row],
                            "rotation": -20.0 * i, "scale": [1.0 + 0.2 * i, 1.0 + 0.2 * i],
                            "opacity": 1.0 - 0.15 * i
                        })
                    }),
                )
                .into(),
            ),
        ),
        Case::new(
            "a count of 0 draws nothing",
            repeated("shape", outlined_box(), json!({ "count": 0 })),
            Expect::Uniform(CLEAR),
        ),
    ]
}

fn one_copy_matches_no_repeat() -> Vec<Case> {
    let offsets = json!({
        "count": 1, "position_offset": [10, 3], "rotation_offset": 20,
        "scale_offset": [0.5, 0.5], "opacity_offset": -0.5
    });
    [("shape", outlined_box()), ("path", triangle())]
        .into_iter()
        .map(|(kind, params)| {
            Case::new(
                format!("a {kind} repeated once draws exactly as without a repeat"),
                repeated(kind, params.clone(), offsets.clone()),
                Expect::Matches(
                    scene(SIZE, json!([{ "id": 1, "type": kind, "params": params }])).into(),
                ),
            )
            .tolerance(0)
        })
        .collect()
}

fn animated_offsets() -> Vec<Case> {
    let animated = |name: &str, motion_blur: f64| {
        let params = with(outlined_box(), json!({ "motion_blur": motion_blur }));
        Case::new(
            name,
            repeated(
                "shape",
                params.clone(),
                json!({ "count": 3, "position_offset": "=vec2(20 + frame * 8, frame * 4)" }),
            )
            .frames(2),
            Expect::Matches(
                separate(
                    "shape",
                    params,
                    (0..3).map(|i| {
                        json!({ "position": format!("=vec2(8 + {i} * (20 + frame * 8), 20 + {i} * frame * 4)") })
                    }),
                )
                .frames(2)
                .into(),
            ),
        )
        .frames([0.0, 1.0])
    };
    vec![
        animated("an animated offset moves the copies each frame", 0.0),
        animated(
            "copies moved by an animated offset blur along their own paths",
            180.0,
        ),
    ]
}

fn frame_count_and_columns_across_the_shutter() -> Vec<Case> {
    // At either end of the shutter, the count would be 1022 and 1026 and the columns -2 and 2.
    let repeats = [
        json!({ "count": "=1024 + frame * 8", "position_offset": [0.05, 0] }),
        json!({ "count": 2, "columns": "=frame * 8", "position_offset": [30, 0] }),
    ];
    let mut cases = Vec::new();
    for (kind, params) in [("shape", outlined_box()), ("path", triangle())] {
        for repeat in &repeats {
            let blurred = with(params.clone(), json!({ "motion_blur": 180 }));
            cases.push(
                Case::new(
                    format!(
                        "a {kind} keeps the frame's count and columns across the shutter, {repeat}"
                    ),
                    repeated(kind, blurred, repeat.clone()),
                    Expect::Matches(repeated(kind, params.clone(), repeat.clone()).into()),
                )
                .tolerance(0),
            );
        }
    }
    cases
}

#[test]
fn repeated_copies_match_the_node_drawn_once_per_copy() {
    check(copies_match_separate_nodes());
}

#[test]
fn a_single_copy_is_identical_to_no_repeat() {
    check(one_copy_matches_no_repeat());
}

#[test]
fn animated_offsets_match_separately_animated_nodes() {
    check(animated_offsets());
}

#[test]
fn motion_blur_keeps_the_frame_count_and_columns() {
    check(frame_count_and_columns_across_the_shutter());
}

/// A path copy scaled almost to nothing reaches far in untransformed space, but the copies
/// beside it keep the reach of their own scale: the repeat records about the union of what each
/// copy records alone, not the whole canvas.
#[test]
fn a_tiny_path_copy_does_not_widen_the_others() {
    let composition = |params: Value| -> Composition {
        scene(
            (1920, 1080),
            json!([{ "id": 1, "type": "path", "params": with(
                json!({ "data": "M0 0 L16 0 L16 12 Z", "position": [100, 200] }),
                params,
            ) }]),
        )
        .into()
    };
    let gpu = gpu();
    let domain = |composition: Composition| {
        let mut renderer = gpu.renderer();
        render(&mut renderer, &composition, 0.0, &TestMedia::default());
        renderer
            .recorded_domain(&composition, NodeId::new(1))
            .unwrap()
            .data_rect
    };
    let normal = domain(composition(json!({})));
    let tiny = domain(composition(json!({ "scale": [0.00001, 1] })));
    let repeated = domain(composition(
        json!({ "repeat": { "count": 2, "scale_offset": [-0.99999, 0] } }),
    ));
    assert!(
        repeated.contains(normal) && repeated.contains(tiny),
        "{repeated:?} should hold {normal:?} and {tiny:?}"
    );
    assert!(
        repeated.width <= 2 * normal.width.max(tiny.width),
        "{repeated:?} is wider than its copies, {normal:?} and {tiny:?}"
    );
}

/// The texture covers the copies that draw anything and no more: three visible copies of a
/// 40 × 20 box, and a fourth at opacity 0, record the domain of one box around the three.
#[test]
fn a_repeat_records_around_its_visible_copies() {
    let composition = |params: Value| -> Composition {
        scene(
            (1920, 1080),
            json!([{ "id": 1, "type": "shape", "params": params }]),
        )
        .into()
    };
    let repeated = composition(json!({
        "width": 40, "height": 20, "position": [100, 200],
        "repeat": { "count": 4, "position_offset": [60, 30], "opacity_offset": -0.4 }
    }));
    let union = composition(json!({ "width": 160, "height": 80, "position": [100, 200] }));
    let gpu = gpu();
    let domain = |composition: &Composition| {
        let mut renderer = gpu.renderer();
        render(&mut renderer, composition, 0.0, &TestMedia::default());
        renderer
            .recorded_domain(composition, NodeId::new(1))
            .unwrap()
    };
    let (repeated, union) = (domain(&repeated), domain(&union));
    assert!(repeated.data_rect.width < 1920, "{:?}", repeated.data_rect);
    assert_eq!(repeated.data_rect, union.data_rect);
    assert_eq!(repeated.storage_size, union.storage_size);
}

/// Tiling only skips copies that cannot reach a texel, so a repeat drawn from its tiles is
/// byte-identical to one drawn from a single tile, here with copies straddling tile borders and
/// overlapping across them.
#[test]
fn tiled_repeats_match_one_tile() {
    let grid = json!({
        "count": 96, "columns": 12, "position_offset": [9, 2], "row_offset": [5, 11],
        "rotation_offset": 13, "scale_offset": [0.01, 0.02], "opacity_offset": -0.004
    });
    let scenes = [
        ("shape", outlined_box()),
        ("path", triangle()),
        (
            "shape",
            with(
                outlined_box(),
                json!({ "position": "=vec2(8 + frame * 6, 20)", "motion_blur": 180 }),
            ),
        ),
    ]
    .map(|(kind, params)| -> Composition {
        scene(
            (160, 120),
            json!([{ "id": 1, "type": kind, "params": with(params, json!({ "repeat": grid })) }]),
        )
        .frames(2)
        .into()
    });
    let gpu = gpu();
    let media = TestMedia::default();
    for (index, composition) in scenes.iter().enumerate() {
        let draw = |single_tile: bool| {
            let mut renderer = gpu.renderer();
            renderer.set_single_tile_repeats(single_tile);
            render(&mut renderer, composition, 1.0, &media)
        };
        let tiled = draw(false);
        assert!(
            tiled.rgba.iter().any(|&byte| byte != 0),
            "scene {index} draws nothing"
        );
        assert!(
            tiled.rgba == draw(true).rgba,
            "scene {index} differs from one tile"
        );
    }
}
