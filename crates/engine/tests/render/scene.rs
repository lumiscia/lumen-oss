//! `render_3d`: where the camera puts objects, what hides what, and when its output is kept.

use lumen_engine::composition::Composition;
use serde_json::{Value, json};

use crate::support::{Case, Expect, Frame, Scene, TestMedia, check, gpu, render, scene};

const SIZE: (u32, u32) = (64, 64);
const CENTER: u32 = 32;
const CLEAR: [u8; 4] = [0, 0, 0, 0];
const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];

/// `scene_nodes`, the last of which is the scene, drawn by a `render_3d` filling the canvas.
fn render_3d(scene_nodes: Vec<Value>) -> Scene {
    let last = scene_nodes.last().expect("a scene node")["id"].clone();
    let mut nodes = scene_nodes;
    nodes.push(json!({ "id": 100, "type": "render_3d", "in": { "scene": last } }));
    scene(SIZE, json!(nodes))
}

/// An unlit shape, which shows its colour exactly wherever it is drawn.
fn flat(id: u32, params: Value, color: [u8; 4]) -> Value {
    let mut params = params;
    params["color"] = json!(color);
    params["unlit"] = json!(true);
    json!({ "id": id, "type": "shape_3d", "params": params })
}

fn merge(id: u32, objects: &[u32], params: Value) -> Value {
    json!({ "id": id, "type": "merge_3d", "params": params, "in": { "objects": objects } })
}

/// Asserts that the white box covers `(x, y)` of `frame`, lit grey.
fn lit_at(frame: &Frame, x: u32, y: u32) {
    let [r, g, b, a] = frame.pixel(x, y);
    assert_eq!(a, 255, "the box covers ({x}, {y})");
    assert!(
        r > 40 && r == g && g == b,
        "a white box is lit grey, got {r} {g} {b}"
    );
}

/// Pixels of row `y` within `xs` that are at least half covered.
fn covered(frame: &Frame, y: u32, xs: std::ops::Range<u32>) -> usize {
    xs.filter(|&x| frame.pixel(x, y)[3] >= 128).count()
}

#[test]
fn a_lit_shape_renders_where_the_camera_puts_it_and_is_transparent_elsewhere() {
    // The default camera is 5 units along +Z with a 40° field of view, so one unit at the origin
    // is about 17.6 pixels of the 64-pixel canvas.
    let lit_box = || json!({ "id": 1, "type": "shape_3d", "params": { "position": [1, 0, 0] } });
    check(vec![
        Case::new(
            "the default camera sees the box right of centre",
            render_3d(vec![lit_box()]),
            Expect::Probes(vec![(14, CENTER, CLEAR), (0, 0, CLEAR), (63, 63, CLEAR)]),
        ),
        Case::new(
            "and lights it",
            render_3d(vec![lit_box()]),
            Expect::Custom(Box::new(|frames| lit_at(&frames[0], 49, CENTER))),
        ),
        Case::new(
            "a camera moved one unit right sees it in the centre",
            render_3d(vec![
                lit_box(),
                json!({ "id": 2, "type": "camera_3d", "params": { "position": [1, 0, 5] } }),
                merge(3, &[1, 2], json!({})),
            ]),
            Expect::Custom(Box::new(|frames| {
                lit_at(&frames[0], CENTER, CENTER);
                assert_eq!(frames[0].pixel(49, CENTER), CLEAR);
            })),
        ),
    ]);
}

#[test]
fn nearer_opaque_geometry_hides_farther_geometry_in_either_node_order() {
    let far = flat(
        1,
        json!({ "size": [2, 2, 0.2], "position": [0, 0, -1] }),
        RED,
    );
    let near = flat(
        2,
        json!({ "size": [0.5, 0.5, 0.2], "position": [0, 0, 1] }),
        GREEN,
    );
    let probes = || Expect::Probes(vec![(CENTER, CENTER, GREEN), (CENTER, 18, RED)]);
    check(vec![
        Case::new(
            "far node first",
            render_3d(vec![
                far.clone(),
                near.clone(),
                merge(3, &[1, 2], json!({})),
            ]),
            probes(),
        ),
        Case::new(
            "near node first",
            render_3d(vec![far, near, merge(3, &[2, 1], json!({}))]),
            probes(),
        ),
    ]);
}

#[test]
fn a_blended_object_shows_what_is_behind_it() {
    let half_green = flat(
        1,
        json!({ "size": [1, 1, 0.2], "position": [0.5, 0, 1], "opacity": 0.5 }),
        GREEN,
    );
    let red = flat(
        2,
        json!({ "size": [1.5, 1, 0.2], "position": [-0.25, 0, -1] }),
        RED,
    );
    // The two overlap just right of centre. Half of each in linear light is sRGB 188.
    check(vec![
        Case::new(
            "the blended box is listed first and still blends over the opaque one",
            render_3d(vec![half_green, red, merge(3, &[1, 2], json!({}))]),
            Expect::Probes(vec![
                (35, CENTER, [188, 188, 0, 255]),
                (48, CENTER, [0, 128, 0, 128]),
                (22, CENTER, RED),
            ]),
        )
        .tolerance(2),
    ]);
}

#[test]
fn orthographic_keeps_sizes_that_perspective_shrinks_with_distance() {
    // Two unit squares facing the camera, the right one 4 units farther away.
    let square = |id, position: [f64; 3]| {
        flat(
            id,
            json!({ "shape": "plane", "rotation": [90, 0, 0], "position": position }),
            GREEN,
        )
    };
    let with_camera = |camera: Value| {
        let mut camera = camera;
        camera["position"] = json!([0, 0, 5]);
        render_3d(vec![
            square(1, [-1.0, 0.0, 0.0]),
            square(2, [1.0, 0.0, -4.0]),
            json!({ "id": 3, "type": "camera_3d", "params": camera }),
            merge(4, &[1, 2, 3], json!({})),
        ])
    };
    let widths = |frames: &[Frame]| {
        (
            covered(&frames[0], CENTER, 0..CENTER),
            covered(&frames[0], CENTER, CENTER..SIZE.0),
        )
    };
    check(vec![
        Case::new(
            "orthographic",
            with_camera(json!({ "projection": "orthographic", "ortho_height": 4 })),
            Expect::Custom(Box::new(move |frames| {
                // 4 units over 64 pixels makes each square 16 pixels wide.
                assert_eq!(widths(frames), (16, 16));
            })),
        ),
        Case::new(
            "perspective",
            with_camera(json!({ "projection": "perspective" })),
            Expect::Custom(Box::new(move |frames| {
                let (near, far) = widths(frames);
                assert!((17..=19).contains(&near), "near square {near} pixels wide");
                assert!((9..=11).contains(&far), "far square {far} pixels wide");
            })),
        ),
    ]);
}

#[test]
fn a_merge_3d_transform_places_its_children() {
    check(vec![Case::new(
        "moved and halved, a child half a unit right lands three quarters right",
        render_3d(vec![
            json!({ "id": 1, "type": "shape_3d", "params": { "position": [0.5, 0, 0] } }),
            merge(
                2,
                &[1],
                json!({ "position": [0.5, 0, 0], "scale": [0.5, 0.5, 0.5] }),
            ),
        ]),
        Expect::Matches(
            render_3d(vec![json!({ "id": 1, "type": "shape_3d", "params": {
                "position": [0.75, 0, 0], "size": [0.5, 0.5, 0.5]
            } })])
            .into(),
        ),
    )]);
}

#[test]
fn rotation_yaws_then_pitches_then_rolls() {
    // A bar along one axis shows which way the rotation turned it: up the image, across it, or
    // end on towards the camera.
    let bar = |size: [f64; 3], rotation: [f64; 3]| {
        render_3d(vec![flat(
            1,
            json!({ "size": size, "rotation": rotation }),
            GREEN,
        )])
    };
    let (above, beside) = ((CENTER, CENTER - 12), (CENTER + 12, CENTER));
    let upright = || Expect::Probes(vec![(above.0, above.1, GREEN), (beside.0, beside.1, CLEAR)]);
    check(vec![
        // Pitching +Z by 90° turns it to -Y, which the yaw then leaves alone. Yawing first
        // would have turned it to +X, across the image.
        Case::new(
            "a bar along Z, yawed and pitched",
            bar([0.2, 0.2, 2.0], [90.0, 90.0, 0.0]),
            upright(),
        ),
        // Rolling +X by 90° turns it to +Y, which the yaw then leaves alone. Yawing first would
        // have turned it towards the camera.
        Case::new(
            "a bar along X, yawed and rolled",
            bar([2.0, 0.2, 0.2], [0.0, 90.0, 90.0]),
            upright(),
        ),
    ]);
}

/// A box under a camera at `position`, aimed at the origin.
fn orbit(position: Value) -> Composition {
    let mut composition = render_3d(vec![
        json!({ "id": 1, "type": "shape_3d", "params": { "rotation": [20, 30, 0] } }),
        json!({ "id": 2, "type": "camera_3d", "params": { "position": position, "look_at": true } }),
        merge(3, &[1, 2], json!({})),
    ])
    .frames(4);
    composition.0["timeline"]["fps"] = json!(4);
    composition.into()
}

#[test]
fn a_render_3d_is_kept_across_frames_until_its_scene_animates() {
    let gpu = gpu();
    let media = TestMedia::default();
    let frames = |composition: &Composition| {
        let mut renderer = gpu.renderer();
        let frames: Vec<Vec<u8>> = (0..4)
            .map(|frame| render(&mut renderer, composition, f64::from(frame), &media).rgba)
            .collect();
        (frames, renderer.skipped_pass_count())
    };

    let (still, skipped) = frames(&orbit(json!([3, 2, 4])));
    assert!(still.iter().all(|frame| *frame == still[0]));
    assert!(
        skipped >= 3 * 2,
        "frames after the first read the kept draw and resolve, skipping {skipped} passes"
    );

    // Only the camera, two nodes below the render, moves.
    let (moving, skipped) = frames(&orbit(json!("=vec3(5 * sin(time), 2, 5 * cos(time))")));
    assert_eq!(skipped, 0, "an animated scene renders every frame");
    for pair in moving.windows(2) {
        assert!(pair[0] != pair[1], "the camera orbits");
    }
}
