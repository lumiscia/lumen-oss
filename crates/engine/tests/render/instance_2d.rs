//! `instance_2d`: copies of a drawing placed by per-instance expressions, drawn as the same
//! drawing placed by hand by `merge_2d` nodes would be.

use lumen_engine::{composition::Composition, gpu::RenderTarget};
use serde_json::{Value, json};

use crate::support::{CLEAR, Case, Expect, Scene, TestMedia, check, gpu, scene};

const SIZE: (u32, u32) = (160, 96);
const STAR: &str = "M 0 -14 L 4 -4 L 14 -4 L 6 3 L 9 13 L 0 7 L -9 13 L -6 3 L -14 -4 L -4 -4 Z";
const SHADOW: &str =
    r#"{ "color": [0, 0, 0, 255], "offset": [3, 4], "radius": 2, "opacity": 0.6 }"#;

fn star(id: u32) -> Value {
    json!({ "id": id, "type": "path_2d", "params": { "data": STAR } })
}

fn fill(id: u32, paint: [u8; 4], opacity: f64) -> Value {
    json!({ "id": id, "type": "fill_2d", "params": { "paint": paint, "opacity": opacity }, "in": { "geometry": 1 } })
}

fn shadow(id: u32, drawing: u32) -> Value {
    let shadow: Value = serde_json::from_str(SHADOW).unwrap();
    json!({ "id": id, "type": "shadow_2d", "params": { "shadows": [shadow] }, "in": { "drawing": drawing } })
}

fn instance(id: u32, drawing: u32, params: Value) -> Value {
    json!({ "id": id, "type": "instance_2d", "params": params, "in": { "drawing": drawing } })
}

/// Copy `index` of the grid: where it sits, its rotation, opacity and tint.
fn grid_copy(index: u32) -> ([f64; 2], f64, f64, [u8; 4]) {
    let i = f64::from(index);
    let position = [
        30.0 + f64::from(index % 3) * 50.0,
        28.0 + f64::from(index / 3) * 40.0,
    ];
    let channel = (40 * index) as u8;
    (
        position,
        f64::from(index / 3) * 30.0,
        1.0 - 0.1 * i,
        [255, 255 - channel, channel, 255],
    )
}

/// A white star with a soft shadow, copied into a three by two grid whose rows turn apart, each
/// copy fading and tinted by its index.
fn grid_by_expression() -> Scene {
    scene(
        SIZE,
        json!([
            star(1),
            fill(2, [255, 255, 255, 255], 1.0),
            shadow(3, 2),
            instance(
                4,
                3,
                json!({
                    "count": 6,
                    "position": "=vec2(30 + (instance % 3) * 50, 28 + floor(instance / 3) * 40)",
                    "rotation": "=floor(instance / 3) * 30",
                    "opacity": "=1 - 0.1 * instance",
                    "tint": "=rgb(255, 255 - 40 * instance, 40 * instance)"
                }),
            ),
            { "id": 5, "type": "render_2d", "in": { "drawing": 4 } }
        ]),
    )
}

/// The same grid, each copy its own fill in the tint's colour and opacity, placed by a `merge_2d`.
fn grid_by_hand() -> Scene {
    let mut nodes = vec![star(1)];
    let mut copies = Vec::new();
    for index in 0..6 {
        let (position, rotation, opacity, paint) = grid_copy(index);
        let id = 10 + 3 * index;
        nodes.push(fill(id, paint, opacity));
        nodes.push(shadow(id + 1, id));
        nodes.push(json!({ "id": id + 2, "type": "merge_2d", "params": { "position": position, "rotation": rotation }, "in": { "drawings": [id + 1] } }));
        copies.push(id + 2);
    }
    nodes.push(json!({ "id": 90, "type": "merge_2d", "in": { "drawings": copies } }));
    nodes.push(json!({ "id": 91, "type": "render_2d", "in": { "drawing": 90 } }));
    scene(SIZE, json!(nodes))
}

#[test]
fn a_grid_of_instances_draws_as_the_same_drawing_placed_by_hand() {
    check(vec![
        Case::new(
            "a grid placed by expressions",
            grid_by_expression(),
            Expect::Matches(grid_by_hand().into()),
        ),
        // The first copy is opaque and tinted yellow.
        Case::new(
            "the grid's first copy",
            grid_by_expression(),
            Expect::Probes(vec![(30, 28, [255, 255, 0, 255]), (5, 90, CLEAR)]),
        ),
        Case::new(
            "no copies",
            scene(
                SIZE,
                json!([
                    star(1),
                    fill(2, [255, 255, 255, 255], 1.0),
                    instance(3, 2, json!({ "count": 0 })),
                    { "id": 4, "type": "render_2d", "in": { "drawing": 3 } }
                ]),
            ),
            Expect::Uniform(CLEAR),
        ),
    ]);
}

/// Copies that only move share their drawables, which the tiles they overlap in still list in
/// draw order: each copy's red fill covers the blue stroke of the copy before it, and its own
/// stroke lies over that copy's fill.
#[test]
fn overlapping_copies_of_a_fill_and_its_stroke_draw_in_copy_order() {
    let red = [255, 0, 0, 255];
    let blue = [0, 0, 255, 255];
    check(vec![Case::new(
        "three overlapping squares",
        rendered(vec![
            json!({ "id": 1, "type": "path_2d", "params": { "data": "M 0 0 L 40 0 L 40 40 L 0 40 Z" } }),
            fill(2, red, 1.0),
            json!({ "id": 3, "type": "stroke_2d", "params": { "paint": blue, "width": 4 }, "in": { "geometry": 1 } }),
            json!({ "id": 4, "type": "merge_2d", "in": { "drawings": [2, 3] } }),
            instance(
                5,
                4,
                json!({ "count": 3, "position": "=vec2(8 + 24 * instance, 8)" }),
            ),
        ]),
        Expect::Probes(vec![
            (8, 28, blue),
            (32, 28, blue),
            (48, 28, red),
            (56, 28, blue),
            (72, 28, red),
            (96, 28, blue),
        ]),
    )]);
}

/// One fill reached through two differently turned `merge_2d`s, under a merge that scales
/// unevenly, draws as two separate fills placed the same way.
#[test]
fn one_fill_turned_two_ways_under_an_uneven_scale_draws_as_two_fills() {
    let triangle = |id: u32| json!({ "id": id, "type": "path_2d", "params": { "data": "M 0 0 L 35 0 L 0 12 Z" } });
    let fill_of = |id: u32, geometry: u32| json!({ "id": id, "type": "fill_2d", "params": { "paint": [255, 0, 0, 255] }, "in": { "geometry": geometry } });
    let turned = |id: u32, drawing: u32, rotation: f64, x: f64| json!({ "id": id, "type": "merge_2d", "params": { "rotation": rotation, "position": [x, 25] }, "in": { "drawings": [drawing] } });
    let scaled = |a: u32, b: u32| json!({ "id": 20, "type": "merge_2d", "params": { "scale": [2, 1] }, "in": { "drawings": [a, b] } });
    let size = (300, 100);
    let separate = scene(
        size,
        json!([
            triangle(1), fill_of(2, 1), triangle(3), fill_of(4, 3),
            turned(10, 2, 45.0, 25.0), turned(11, 4, 60.0, 100.0), scaled(10, 11),
            { "id": 100, "type": "render_2d", "in": { "drawing": 20 } }
        ]),
    );
    check(vec![Case::new(
        "a shared fill turned 45 and 60 degrees",
        scene(
            size,
            json!([
                triangle(1), fill_of(2, 1),
                turned(10, 2, 45.0, 25.0), turned(11, 2, 60.0, 100.0), scaled(10, 11),
                { "id": 100, "type": "render_2d", "in": { "drawing": 20 } }
            ]),
        ),
        Expect::Matches(separate.into()),
    )]);
}

/// `drawing` drawn through a `render_2d`, as the scene's last nodes.
fn rendered(mut nodes: Vec<Value>) -> Scene {
    let last = nodes.last().expect("a drawing node")["id"].clone();
    nodes.push(json!({ "id": 100, "type": "render_2d", "in": { "drawing": last } }));
    scene(SIZE, json!(nodes))
}

#[test]
fn copies_beside_the_params_they_replace_fail_check_naming_them() {
    let scene = rendered(vec![
        star(1),
        fill(2, [255, 255, 255, 255], 1.0),
        instance(
            3,
            2,
            json!({ "copies": [{ "position": [10, 10] }], "count": 2, "tint": "#ff0000" }),
        ),
    ]);
    let error = match lumen_engine::json::parse_value(&scene.0) {
        Ok(_) => panic!("the composition parses"),
        Err(error) => format!("{error:#}"),
    };
    assert!(
        error.contains("can't be set together with `count`, `tint`"),
        "{error}"
    );
}

fn render_error(nodes: Vec<Value>) -> String {
    let composition: Composition = rendered(nodes).into();
    gpu()
        .renderer()
        .render(
            &composition,
            0.0,
            &TestMedia::default(),
            RenderTarget::Owned,
        )
        .expect_err("the render fails")
        .to_string()
}

#[test]
fn nested_instances_past_the_limits_fail_naming_the_node() {
    let mut nodes = vec![star(1), fill(2, [255, 255, 255, 255], 1.0)];
    for id in 3..=11 {
        nodes.push(instance(id, id - 1, json!({ "count": 1 })));
    }
    let error = render_error(nodes);
    assert!(error.contains("nested more than 8 deep"), "{error}");

    // Nested counts multiply: 200 copies of 200 copies are more drawables than a drawing holds.
    let error = render_error(vec![
        star(1),
        fill(2, [255, 255, 255, 255], 1.0),
        instance(3, 2, json!({ "count": 200 })),
        instance(4, 3, json!({ "count": 200 })),
    ]);
    assert!(error.contains("node 4"), "{error}");
    assert!(
        error.contains(
            "200 copies of a drawing of 200 drawables make a drawing of more than 16384 drawables"
        ),
        "{error}"
    );
}
