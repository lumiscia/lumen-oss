//! Templates whose output is a scene: clips playing them inside a `merge_3d`, each with its own
//! props and time, what a scene capture reports of them, mattes named inside them, and particles
//! placed inside them.

use lumen_engine::{composition::Composition, node::NodeId};
use serde_json::{Value, json};

use crate::support::scene::{
    SIZE, flat, listed, merge, model, quads, render_3d, render_3d_with, unlit,
};
use crate::support::{Frame, Scene, TestMedia, gpu, render, render_working_space};

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];

/// A template of an unlit box whose colour is the prop `color` and whose x is the prop `x` plus
/// `drift` units a frame, which names the matte `matte`.
fn card(drift: f64, matte: &str) -> Value {
    json!({
        "timeline": { "fps": 30, "duration_frames": 10 },
        "props": {
            "color": { "type": "color", "default": "#ffffff" },
            "x": { "type": "number", "default": 0 }
        },
        "nodes": [
            { "id": 1, "type": "material_3d", "params": { "color": "=prop('color')", "unlit": true } },
            { "id": 2, "type": "shape_3d", "params": {
                "position": format!("=vec3(prop('x') + frame * {drift}, 0, 0)"),
                "matte": matte,
            } },
            { "id": 3, "type": "media_output" }
        ],
        "connections": [
            { "from_node": 1, "to_node": 2, "to_port": "material" },
            { "from_node": 2, "to_node": 3, "to_port": "source" }
        ]
    })
}

fn clip(id: u32, params: Value) -> Value {
    let mut params = params;
    params["composition"] = json!("card");
    json!({ "id": id, "type": "clip", "params": params })
}

/// The scene of `objects` merged, with `card` among the root's templates.
fn with_card(card: Value, objects: Vec<Value>, render: Value) -> Scene {
    let ids: Vec<u32> = objects
        .iter()
        .map(|object| object["id"].as_u64().unwrap() as u32)
        .collect();
    let mut nodes = objects;
    nodes.push(merge(20, &ids, json!({})));
    let mut scene = render_3d_with(render, nodes).frames(10);
    scene.0["compositions"] = json!({ "card": card });
    scene
}

fn two_cards() -> Scene {
    with_card(
        card(0.0, ""),
        vec![
            clip(10, json!({ "props": { "color": "#ff0000", "x": -1 } })),
            clip(11, json!({ "props": { "color": "#00ff00", "x": 1 } })),
        ],
        json!({}),
    )
}

#[test]
fn two_clips_of_a_scene_template_draw_as_the_same_scene_written_inline() {
    let inline = render_3d(listed(
        &[
            flat(
                1,
                11,
                json!({ "position": [-1, 0, 0] }),
                json!({ "color": RED }),
            ),
            flat(
                2,
                12,
                json!({ "position": [1, 0, 0] }),
                json!({ "color": GREEN }),
            ),
        ],
        merge(20, &[1, 2], json!({})),
    ));
    let gpu = gpu();
    let media = TestMedia::default();
    let expected = render(&mut gpu.renderer(), &inline.into(), 0.0, &media);
    let actual = render(&mut gpu.renderer(), &two_cards().into(), 0.0, &media);
    assert!(expected.rgba.contains(&255), "the boxes draw");
    assert_eq!(actual.pixel(16, 32), RED);
    assert_eq!(actual.pixel(48, 32), GREEN);
    assert!(
        actual.rgba == expected.rgba,
        "the clips draw as the inline scene"
    );
}

#[test]
fn a_scene_template_plays_in_its_clips_time_with_props_in_the_parents() {
    // The prop moves in the root's frames, the box's own drift in the template's, which the clip
    // starts at frame 2 from the template's frame 1.
    let templated = with_card(
        card(0.05, ""),
        vec![clip(
            10,
            json!({ "start": 2, "offset": 1, "props": { "color": "#ff0000", "x": "=-1 + frame * 0.1" } }),
        )],
        json!({}),
    );
    let inline = render_3d(listed(
        &[flat(
            1,
            11,
            json!({ "position": "=vec3(-1 + frame * 0.1 + (frame - 1) * 0.05, 0, 0)" }),
            json!({ "color": RED }),
        )],
        merge(20, &[1], json!({})),
    ))
    .frames(10);
    let gpu = gpu();
    let media = TestMedia::default();
    let (templated, inline): (Composition, Composition) = (templated.into(), inline.into());
    let mut renderer = gpu.renderer();
    let hidden = render(&mut renderer, &templated, 1.0, &media);
    assert!(
        hidden.rgba.chunks(4).all(|pixel| pixel[3] == 0),
        "nothing shows before the clip starts"
    );
    for frame in [2.0, 5.0, 9.0] {
        let expected = render(&mut gpu.renderer(), &inline, frame, &media);
        let actual = render(&mut renderer, &templated, frame, &media);
        assert!(expected.rgba.contains(&255), "the box draws at {frame}");
        assert!(actual.rgba == expected.rgba, "frame {frame} matches");
    }
}

#[test]
fn a_capture_reports_each_clips_objects_with_the_template_and_clip_they_are_in() {
    let mut renderer = gpu().renderer();
    renderer.set_scene_capture(true);
    let composition: Composition = two_cards().into();
    render(&mut renderer, &composition, 0.0, &TestMedia::default());
    let [capture] = renderer
        .take_scene_captures()
        .try_into()
        .expect("one capture");
    let objects: Vec<_> = capture
        .objects
        .iter()
        .map(|object| {
            (
                object.node,
                object.template.as_deref(),
                object.clips.clone(),
                object.world[12],
            )
        })
        .collect();
    assert_eq!(
        objects,
        [
            (NodeId::new(2), Some("card"), vec![NodeId::new(10)], -1.0),
            (NodeId::new(2), Some("card"), vec![NodeId::new(11)], 1.0),
        ]
    );
}

#[test]
fn a_matte_named_inside_a_scene_template_takes_its_objects() {
    let composition: Composition = with_card(
        card(0.0, "cards"),
        vec![clip(10, json!({ "props": { "x": -1 } }))],
        json!({ "output": "matte", "mattes": ["other", "cards"] }),
    )
    .into();
    let texels = render_working_space(
        &mut gpu().renderer(),
        &composition,
        0.0,
        &TestMedia::default(),
    );
    let at = |x: u32| texels[(32 * SIZE.0 + x) as usize];
    assert_eq!(at(16)[..2], [0.0, 1.0], "the box takes the second channel");
    assert_eq!(at(48)[..2], [0.0, 0.0], "nothing is right of it");
}

/// How many pixels of `frame` are drawn left of its middle, and how many right of it.
fn sides(frame: &Frame) -> (usize, usize) {
    let drawn: Vec<u32> = (0..SIZE.1)
        .flat_map(|y| (0..SIZE.0).map(move |x| (x, y)))
        .filter(|&(x, y)| frame.pixel(x, y)[3] > 0)
        .map(|(x, _)| x)
        .collect();
    let left = drawn.iter().filter(|&&x| x < SIZE.0 / 2).count();
    (left, drawn.len() - left)
}

/// A template of boxes scattered over the model `source`, which is left out.
fn scatter_over(source: &str) -> Value {
    json!({
        "timeline": { "fps": 30, "duration_frames": 1 },
        "nodes": [
            model(1, json!({ "source": source })),
            { "id": 3, "type": "material_3d", "params": { "unlit": true } },
            { "id": 4, "type": "shape_3d", "params": { "size": [0.1, 0.1, 0.1] } },
            { "id": 2, "type": "particles_3d", "params": { "count": 30, "seed": 3 } },
            { "id": 5, "type": "media_output" }
        ],
        "connections": [
            { "from_node": 3, "to_node": 4, "to_port": "material" },
            { "from_node": 1, "to_node": 2, "to_port": "surface" },
            { "from_node": 4, "to_node": 2, "to_port": "object" },
            { "from_node": 2, "to_node": 5, "to_port": "source" }
        ]
    })
}

#[test]
fn particles_in_two_templates_place_over_their_own_surfaces() {
    // The templates differ only in their model, so their particles' params and placements match.
    let material = json!([unlit("white", [1.0, 1.0, 1.0, 1.0])]);
    let media = TestMedia::default()
        .model(
            "left.glb",
            quads(material.clone(), &[([-2.0, -1.0, -1.0, 1.0], 0)]),
        )
        .model("right.glb", quads(material, &[([1.0, -1.0, 2.0, 1.0], 0)]));
    let clip = |id: u32, composition: &str, start: u32| {
        json!({ "id": id, "type": "clip",
            "params": { "composition": composition, "start": start, "duration": 1 } })
    };
    let mut scene = render_3d_with(
        json!({}),
        vec![
            clip(10, "a", 0),
            clip(11, "b", 1),
            merge(20, &[10, 11], json!({})),
        ],
    )
    .frames(2);
    scene.0["compositions"] =
        json!({ "a": scatter_over("left.glb"), "b": scatter_over("right.glb") });
    let composition: Composition = scene.into();
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let (left, right) = sides(&render(&mut renderer, &composition, 0.0, &media));
    assert!(
        left > 0 && right == 0,
        "frame 0 scatters over the left model: {left}, {right}"
    );
    let second = render(&mut renderer, &composition, 1.0, &media);
    let (left, right) = sides(&second);
    assert!(
        left == 0 && right > 0,
        "frame 1 scatters over the right model: {left}, {right}"
    );
    let fresh = render(&mut gpu.renderer(), &composition, 1.0, &media);
    assert!(
        second.rgba == fresh.rgba,
        "frame 1 draws as it does on its own"
    );
}

#[test]
fn particles_in_two_clips_of_one_template_place_over_each_clips_surface() {
    // Each clip extrudes a square the prop `x` moves, so the surfaces differ only in their
    // vertices. The first clip shows at frame 0 alone; at frame 1 the second is the first to
    // place particles, as the first did the frame before.
    let slab = json!({
        "timeline": { "fps": 30, "duration_frames": 2 },
        "props": { "x": { "type": "number", "default": 0 } },
        "nodes": [
            { "id": 6, "type": "shape_2d", "params": {
                "width": 100, "height": 100, "position": "=vec2(prop('x'), -50)" } },
            { "id": 1, "type": "extrude_3d" },
            { "id": 3, "type": "material_3d", "params": { "unlit": true } },
            { "id": 4, "type": "shape_3d", "params": { "size": [0.1, 0.1, 0.1] } },
            { "id": 2, "type": "particles_3d", "params": { "count": 30, "seed": 3 } },
            { "id": 5, "type": "media_output" }
        ],
        "connections": [
            { "from_node": 6, "to_node": 1, "to_port": "geometry" },
            { "from_node": 3, "to_node": 4, "to_port": "material" },
            { "from_node": 1, "to_node": 2, "to_port": "surface" },
            { "from_node": 4, "to_node": 2, "to_port": "object" },
            { "from_node": 2, "to_node": 5, "to_port": "source" }
        ]
    });
    let clip = |id: u32, duration: u32, x: i32| {
        json!({ "id": id, "type": "clip", "params": { "composition": "slab",
            "duration": duration, "props": { "x": x } } })
    };
    let mut scene = render_3d_with(
        json!({}),
        vec![
            clip(10, 1, -150),
            clip(11, 2, 50),
            merge(20, &[10, 11], json!({})),
        ],
    )
    .frames(2);
    scene.0["compositions"] = json!({ "slab": slab });
    let composition: Composition = scene.into();
    let gpu = gpu();
    let media = TestMedia::default();
    let mut renderer = gpu.renderer();
    let (left, right) = sides(&render(&mut renderer, &composition, 0.0, &media));
    assert!(
        left > 0 && right > 0,
        "frame 0 scatters over both squares: {left}, {right}"
    );
    let second = render(&mut renderer, &composition, 1.0, &media);
    let (left, right) = sides(&second);
    assert!(
        left == 0 && right > 0,
        "frame 1 scatters over the right square: {left}, {right}"
    );
    let fresh = render(&mut gpu.renderer(), &composition, 1.0, &media);
    assert!(
        second.rgba == fresh.rgba,
        "frame 1 draws as it does on its own"
    );
}
