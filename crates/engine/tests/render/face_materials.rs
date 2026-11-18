//! Per-face materials: `shape_3d` boxes and the sides of `text_3d` and `extrude_3d`.

use lumen_engine::{composition::Composition, gpu::RenderTarget};
use serde_json::{Value, json};

use crate::support::scene::{material, merge, ortho, render_3d};
use crate::support::{BLUE, Frame, GREEN, RED, Scene, TestMedia, gpu, render};

fn unlit(id: u32, color: [u8; 4]) -> Value {
    material(id, json!({ "color": color, "unlit": true }))
}

fn node(id: u32, kind: &str, params: Value, inputs: Value) -> Value {
    json!({ "id": id, "type": kind, "params": params, "in": inputs })
}

/// `object` seen from above and in front, which shows its top and front, in a 64 pixel frame
/// where one unit is 16 pixels: the middle of its top is 6 pixels above the centre, and the
/// middle of its front 6 pixels below.
fn from_above_in_front(object: Vec<Value>, last: u32) -> Scene {
    let mut nodes = object;
    nodes.push(ortho(90, json!({ "position": [0, 4, 4], "look_at": true })));
    nodes.push(merge(91, &[last, 90], json!({})));
    render_3d(nodes)
}

fn frame(scene: Scene) -> Frame {
    render(
        &mut gpu().renderer(),
        &scene.into(),
        0.0,
        &TestMedia::default(),
    )
}

#[test]
fn a_boxs_faces_take_their_own_materials_and_the_rest_take_material() {
    // From the right, above and in front, the top, right and front faces show, around the
    // centre: 6 pixels above it, 6 right and 3 below, and 6 left and 3 below.
    let scene = render_3d(vec![
        unlit(1, RED),
        unlit(2, GREEN),
        unlit(3, BLUE),
        node(
            4,
            "shape_3d",
            json!({}),
            json!({ "material": 3, "top_material": 1, "right_material": 2 }),
        ),
        ortho(5, json!({ "position": [4, 4, 4], "look_at": true })),
        merge(6, &[4, 5], json!({})),
    ]);
    let frame = frame(scene);
    assert_eq!(frame.pixel(32, 25), RED, "top");
    assert_eq!(frame.pixel(38, 35), GREEN, "right");
    assert_eq!(frame.pixel(26, 35), BLUE, "front falls back to `material`");
}

#[test]
fn an_extrusions_walls_take_side_material_and_its_caps_material() {
    let frame = frame(from_above_in_front(
        vec![
            node(
                1,
                "shape_2d",
                json!({ "width": 100, "height": 100, "position": [-50, -50] }),
                json!({}),
            ),
            unlit(2, RED),
            unlit(3, GREEN),
            node(
                4,
                "extrude_3d",
                json!({ "depth": 1, "bevel_size": 0 }),
                json!({ "geometry": 1, "material": 3, "side_material": 2 }),
            ),
        ],
        4,
    ));
    assert_eq!(frame.pixel(32, 26), RED, "the top wall");
    assert_eq!(frame.pixel(32, 38), GREEN, "the front cap");
}

#[test]
fn each_glyph_of_a_text_keeps_its_walls_and_caps_in_their_materials() {
    let frame = frame(from_above_in_front(
        vec![
            unlit(1, RED),
            unlit(2, GREEN),
            node(
                3,
                "text_3d",
                json!({ "content": "ll", "font_size": 2.5, "depth": 1 }),
                json!({ "material": 2, "side_material": 1 }),
            ),
        ],
        3,
    ));
    // The columns the glyphs cover, grouped into one run for each stem.
    let shown = &frame;
    let covered = |x: u32| (0..shown.height).filter(move |&y| shown.pixel(x, y)[3] == 255);
    let mut stems: Vec<Vec<u32>> = Vec::new();
    let mut last = None;
    for x in (0..frame.width).filter(|&x| covered(x).next().is_some()) {
        if last.is_some_and(|last| last + 1 == x) {
            stems.last_mut().unwrap().push(x);
        } else {
            stems.push(vec![x]);
        }
        last = Some(x);
    }
    assert_eq!(stems.len(), 2, "two stems: {stems:?}");
    for stem in stems {
        let x = stem[stem.len() / 2];
        let (top, bottom) = (covered(x).next().unwrap(), covered(x).next_back().unwrap());
        assert_eq!(frame.pixel(x, top + 3), RED, "the top wall at column {x}");
        assert_eq!(
            frame.pixel(x, bottom - 3),
            GREEN,
            "the front cap at column {x}"
        );
    }
}

/// Four boxes along X with a red top, the rest blue, placed by an `instance_3d` or by hand.
fn row_of_boxes(instanced: bool) -> Scene {
    let inputs = json!({ "material": 2, "top_material": 1 });
    let mut nodes = vec![unlit(1, RED), unlit(2, BLUE)];
    let ids: Vec<u32> = if instanced {
        nodes.push(node(
            3,
            "shape_3d",
            json!({ "size": [0.8, 0.8, 0.8] }),
            inputs,
        ));
        nodes.push(node(
            4,
            "instance_3d",
            json!({ "count": 4, "position": "=vec3(instance * 1.2 - 1.8, 0, 0)" }),
            json!({ "scene": 3 }),
        ));
        vec![4]
    } else {
        (0..4)
            .map(|i| {
                let params =
                    json!({ "size": [0.8, 0.8, 0.8], "position": [i as f64 * 1.2 - 1.8, 0, 0] });
                nodes.push(node(10 + i, "shape_3d", params, inputs.clone()));
                10 + i
            })
            .collect()
    };
    nodes.push(ortho(90, json!({ "position": [0, 4, 4], "look_at": true })));
    nodes.push(merge(91, &[&ids[..], &[90]].concat(), json!({})));
    render_3d(nodes)
}

#[test]
fn instanced_boxes_with_face_materials_draw_as_the_same_boxes_placed_by_hand() {
    let (instanced, by_hand) = (frame(row_of_boxes(true)), frame(row_of_boxes(false)));
    let count = |frame: &Frame, color: [u8; 4]| {
        frame
            .rgba
            .chunks(4)
            .filter(|pixel| **pixel == color)
            .count()
    };
    // Each box shows its top and front, which are about 9 by 9 pixels.
    assert!(
        count(&instanced, RED) > 4 * 40,
        "{}",
        count(&instanced, RED)
    );
    assert!(
        count(&instanced, BLUE) > 4 * 40,
        "{}",
        count(&instanced, BLUE)
    );
    assert_eq!(instanced.rgba, by_hand.rgba);
}

#[test]
fn a_face_material_on_another_shape_fails_naming_the_input() {
    let composition: Composition = render_3d(vec![
        unlit(1, RED),
        node(
            2,
            "shape_3d",
            json!({ "shape": "sphere" }),
            json!({ "top_material": 1 }),
        ),
    ])
    .into();
    let error = gpu()
        .renderer()
        .render(
            &composition,
            0.0,
            &TestMedia::default(),
            RenderTarget::Owned,
        )
        .expect_err("the render fails")
        .to_string();
    assert!(
        error.contains("node 2") && error.contains("`top_material`"),
        "{error}"
    );
}
