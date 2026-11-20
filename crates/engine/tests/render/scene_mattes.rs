//! `render_3d`'s `matte` output: which objects' coverage each channel holds, how surfaces in front
//! hide it, its antialiasing, that it draws with the colour, and `merge` reading one channel.

use lumen_engine::{composition::Composition, gpu::RenderTarget};
use serde_json::{Value, json};

use crate::support::scene::{SIZE, material, merge, with_material};
use crate::support::{Scene, TestMedia, gpu, render, render_working_space, scene};

/// A box `size` across at `position`, taking the matte `matte`.
fn cube(id: u32, position: [f64; 3], size: f64, matte: &str) -> Value {
    json!({ "id": id, "type": "shape_3d", "params": {
        "position": position, "size": [size, size, size], "matte": matte,
    } })
}

/// `scene_nodes`, the last of which is the scene, drawn by the `render_3d` 100 with `params`.
fn rendered(params: Value, scene_nodes: Vec<Value>) -> Vec<Value> {
    let last = scene_nodes.last().expect("a scene node")["id"].clone();
    let mut nodes = scene_nodes;
    nodes
        .push(json!({ "id": 100, "type": "render_3d", "params": params, "in": { "scene": last } }));
    nodes
}

/// The texels of the matte `render_3d` 100 outputs over `scene_nodes` with `mattes`.
fn matte(mattes: Value, scene_nodes: Vec<Value>) -> Vec<[f32; 4]> {
    let params = json!({ "output": "matte", "mattes": mattes });
    let composition: Composition = scene(SIZE, json!(rendered(params, scene_nodes))).into();
    render_working_space(
        &mut gpu().renderer(),
        &composition,
        0.0,
        &TestMedia::default(),
    )
}

fn at(texels: &[[f32; 4]], x: u32, y: u32) -> [f32; 4] {
    texels[(y * SIZE.0 + x) as usize]
}

fn near(texel: [f32; 4], expected: [f32; 4], tolerance: f32, what: &str) {
    assert!(
        texel
            .iter()
            .zip(expected)
            .all(|(value, expected)| (value - expected).abs() <= tolerance),
        "{what}: expected {expected:?}, got {texel:?}"
    );
}

/// Box `a` on the left in front of box `b` on the right, which it covers where they overlap, and
/// a small box `d` above them.
fn three_cubes() -> Vec<Value> {
    vec![
        cube(1, [-0.5, 0.0, 1.0], 1.0, "a"),
        cube(2, [0.6, 0.0, -1.0], 1.6, "b"),
        cube(3, [0.0, 1.2, 0.0], 0.4, "d"),
        merge(4, &[1, 2, 3], json!({})),
    ]
}

#[test]
fn each_channel_holds_the_visible_coverage_of_the_objects_its_name_selects() {
    let gpu = gpu();
    let params = json!({ "output": "matte", "mattes": ["a", "b", "unused", "d"] });
    let composition: Composition = scene(SIZE, json!(rendered(params, three_cubes()))).into();
    let mut renderer = gpu.renderer();
    let texels = render_working_space(&mut renderer, &composition, 0.0, &TestMedia::default());
    near(at(&texels, 15, 32), [1.0, 0.0, 0.0, 0.0], 0.0, "box a");
    near(at(&texels, 45, 32), [0.0, 1.0, 0.0, 0.0], 0.0, "box b");
    near(
        at(&texels, 30, 32),
        [1.0, 0.0, 0.0, 0.0],
        0.0,
        "box a hiding box b",
    );
    near(
        at(&texels, 32, 10),
        [0.0, 0.0, 0.0, 1.0],
        0.0,
        "box d, in alpha",
    );
    near(at(&texels, 2, 2), [0.0; 4], 0.0, "nothing");

    let warnings = renderer.take_diagnostics();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].code, "scene.matte_unused");
    assert!(warnings[0].message.contains("`unused`"), "{warnings:?}");
}

#[test]
fn a_surface_in_front_hides_a_matte_by_its_alpha() {
    let glass = with_material(
        json!({ "id": 2, "type": "shape_3d", "params": {
            "shape": "plane", "size": [4, 1, 4], "rotation": [90, 0, 0], "position": [0, 0, 2],
        } }),
        5,
    );
    let texels = matte(
        json!(["a"]),
        vec![
            material(5, json!({ "opacity": 0.5 })),
            cube(1, [0.0, 0.0, 0.0], 1.0, "a"),
            glass,
            merge(3, &[1, 2], json!({})),
        ],
    );
    near(
        at(&texels, 32, 32),
        [0.5, 0.0, 0.0, 0.0],
        0.01,
        "behind a plane at 0.5 opacity",
    );
}

#[test]
fn a_merge_3d_names_the_matte_of_objects_inside_that_name_none_of_their_own() {
    let texels = matte(
        json!(["group", "own"]),
        vec![
            cube(1, [-0.8, 0.0, 0.0], 1.0, ""),
            cube(2, [0.8, 0.0, 0.0], 1.0, "own"),
            merge(3, &[1, 2], json!({ "matte": "group" })),
        ],
    );
    near(
        at(&texels, 17, 32),
        [1.0, 0.0, 0.0, 0.0],
        0.0,
        "the unnamed box",
    );
    near(
        at(&texels, 47, 32),
        [0.0, 1.0, 0.0, 0.0],
        0.0,
        "the named box",
    );
}

#[test]
fn a_mattes_edges_are_antialiased_as_the_colours_alpha_is() {
    let sphere = || {
        vec![json!({ "id": 1, "type": "shape_3d", "params": {
            "shape": "sphere", "size": [2.5, 2.5, 2.5], "matte": "a",
        } })]
    };
    let texels = |output: &str| {
        let params = json!({ "output": output, "mattes": ["a"], "supersample": 4 });
        let composition: Composition = scene(SIZE, json!(rendered(params, sphere()))).into();
        render_working_space(
            &mut gpu().renderer(),
            &composition,
            0.0,
            &TestMedia::default(),
        )
    };
    let (color, matte) = (texels("color"), texels("matte"));
    let mut edges = 0;
    for (color, matte) in color.iter().zip(&matte) {
        assert!(
            (color[3] - matte[0]).abs() <= 0.002,
            "the colour's alpha {} against the matte's {}",
            color[3],
            matte[0]
        );
        edges += usize::from(0.1 < matte[0] && matte[0] < 0.9);
    }
    assert!(edges > 20, "{edges} pixels part covered");
}

/// The boxes drawn by node 100, under white through a `merge` masked by its `mask` output read
/// as `mask_mode`.
fn masked_by(mask: &str, mask_mode: &str) -> Scene {
    let mut nodes = rendered(json!({ "mattes": ["a", "b"] }), three_cubes());
    nodes.extend([
        json!({ "id": 9, "type": "background", "params": { "paint": [255, 255, 255, 255] } }),
        json!({ "id": 10, "type": "merge", "params": { "mask_mode": mask_mode }, "in": { "overlay": 9 } }),
    ]);
    let mut composition = scene(SIZE, json!(nodes));
    composition.0["connections"]
        .as_array_mut()
        .expect("connections")
        .extend([
            json!({ "from_node": 100, "from_port": "color", "to_node": 10, "to_port": "base" }),
            json!({ "from_node": 100, "from_port": mask, "to_node": 10, "to_port": "mask" }),
        ]);
    composition
}

#[test]
fn colour_and_matte_read_from_one_render_share_its_walk_and_draw() {
    let gpu = gpu();
    let draw = |composition: Scene| {
        let mut renderer = gpu.renderer();
        let frame = render(
            &mut renderer,
            &composition.into(),
            0.0,
            &TestMedia::default(),
        );
        (
            frame,
            renderer.scene_walk_count(),
            renderer.submitted_pass_count(),
        )
    };
    let (green, walks, passes) = draw(masked_by("matte", "green"));
    assert_eq!(walks, 1);
    // Read beside the colour, the mattes cost the resolve that stores them and no draw.
    let (_, _, colour_alone) = draw(masked_by("color", "green"));
    assert_eq!(passes, colour_alone + 1);

    let colour = draw(scene(
        SIZE,
        json!(rendered(json!({ "mattes": ["a", "b"] }), three_cubes())),
    ))
    .0;
    let white = [255, 255, 255, 255];
    assert_eq!(green.pixel(45, 32), white, "green selects box b");
    assert_eq!(
        green.pixel(15, 32),
        colour.pixel(15, 32),
        "and leaves box a"
    );
    let (red, _, _) = draw(masked_by("matte", "red"));
    assert_eq!(red.pixel(15, 32), white, "red selects box a");
    assert_eq!(red.pixel(45, 32), colour.pixel(45, 32), "and leaves box b");
}

#[test]
fn mattes_listing_more_than_four_names_or_a_name_twice_fail_the_render() {
    for (mattes, expected) in [
        (
            json!(["a", "b", "c", "d", "e"]),
            "`mattes` lists 5 names, more than the 4 channels",
        ),
        (json!(["a", "b", "a"]), "`mattes` lists `a` twice"),
    ] {
        let nodes = rendered(json!({ "mattes": mattes }), three_cubes());
        let composition: Composition = scene(SIZE, json!(nodes)).into();
        let error = gpu()
            .renderer()
            .render(
                &composition,
                0.0,
                &TestMedia::default(),
                RenderTarget::Owned,
            )
            .expect_err("the render fails");
        assert!(error.to_string().contains(expected), "{error}");
    }
}
