//! `particles_3d`: copies of a scene scattered over a surface and emitted over time, placed on the
//! GPU from the seed and the time alone.

use lumen_engine::{composition::Composition, gpu::RenderTarget};
use serde_json::{Value, json};

use crate::support::{CLEAR, Case, Expect, Frame, TestMedia, check, gpu, render, scene};

const SIZE: (u32, u32) = (96, 96);

fn shape(id: u32, params: Value) -> Value {
    json!({ "id": id, "type": "shape_3d", "params": params })
}

fn particles(id: u32, surface: Option<u32>, object: u32, params: Value) -> Value {
    let mut node = json!({ "id": id, "type": "particles_3d", "params": params,
        "in": { "object": object } });
    if let Some(surface) = surface {
        node["in"]["surface"] = json!(surface);
    }
    node
}

/// `nodes` and a camera, merged and drawn by a `render_3d` filling the canvas, over 90 frames.
fn render_3d(mut nodes: Vec<Value>, shown: &[u32], camera: Value) -> crate::support::Scene {
    nodes.push(json!({ "id": 80, "type": "camera_3d", "params": camera }));
    let mut ids = shown.to_vec();
    ids.push(80);
    nodes.push(json!({ "id": 90, "type": "merge_3d", "in": { "objects": ids } }));
    nodes.push(
        json!({ "id": 100, "type": "render_3d", "params": { "environment": 0 },
        "in": { "scene": 90 } }),
    );
    scene(SIZE, json!(nodes)).frames(90)
}

fn front() -> Value {
    json!({ "position": [0, 1, 8], "look_at": true, "target": [0, 1, 0] })
}

/// Looks straight down at the ground, 8 units across.
fn above() -> Value {
    json!({ "projection": "orthographic", "ortho_height": 8, "position": [0, 10, 0],
        "rotation": [-90, 0, 0] })
}

/// Particles from the origin moving up at a unit a second, one born a second, drawn as boxes.
fn rising(params: Value) -> crate::support::Scene {
    let mut all = json!({ "count": 10, "rate": 1, "speed": 1 });
    all.as_object_mut()
        .unwrap()
        .extend(params.as_object().unwrap().clone());
    render_3d(
        vec![
            shape(1, json!({ "size": [0.4, 0.4, 0.4] })),
            particles(2, None, 1, all),
        ],
        &[2],
        front(),
    )
}

/// Boxes like `rising`'s, placed by hand at these heights.
fn boxes_at(heights: &[f64]) -> crate::support::Scene {
    let nodes: Vec<Value> = (0..)
        .zip(heights)
        .map(|(i, &y)| {
            shape(
                10 + i,
                json!({ "size": [0.4, 0.4, 0.4], "position": [0, y, 0] }),
            )
        })
        .collect();
    let ids: Vec<u32> = (10..10 + nodes.len() as u32).collect();
    render_3d(nodes, &ids, front())
}

#[test]
fn particles_are_born_one_by_one_and_rise_and_fall_by_the_formula() {
    check(vec![
        Case::new(
            "only the first is born at 0 seconds",
            rising(json!({})),
            Expect::Matches(boxes_at(&[0.0]).into()),
        ),
        Case::new(
            "two seconds in, three have risen for 2, 1 and 0 seconds",
            rising(json!({})),
            Expect::Matches(boxes_at(&[0.0, 1.0, 2.0]).into()),
        )
        .frames([60.0]),
        Case::new(
            "gravity pulls a particle down by half its pull times its age squared",
            rising(
                json!({ "count": 1, "rate": 0, "speed": 0, "gravity": [0, -2, 0],
                "lifetime": 1.5 }),
            ),
            Expect::Matches(boxes_at(&[-1.0]).into()),
        )
        .frames([30.0]),
        Case::new(
            "and past its lifetime it is gone",
            rising(
                json!({ "count": 1, "rate": 0, "speed": 0, "gravity": [0, -2, 0],
                "lifetime": 1.5 }),
            ),
            Expect::Uniform(CLEAR),
        )
        .frames([60.0]),
        Case::new(
            "under an instance_3d of one copy, which places them as a group",
            {
                let mut scene = rising(json!({}));
                let nodes = scene.0["nodes"].as_array_mut().unwrap();
                nodes.push(json!({ "id": 5, "type": "instance_3d",
                    "params": { "position": [1, 0, 0] } }));
                scene.0["connections"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .for_each(|connection| {
                        if connection["from_node"] == 2 && connection["to_node"] == 90 {
                            connection["from_node"] = json!(5);
                        }
                    });
                scene.0["connections"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({ "from_node": 2, "to_node": 5, "to_port": "scene" }));
                scene
            },
            Expect::Matches({
                let mut scene = boxes_at(&[0.0, 1.0, 2.0]);
                for node in scene.0["nodes"].as_array_mut().unwrap() {
                    if node["type"] == "shape_3d" {
                        node["params"]["position"][0] = json!(1);
                    }
                }
                scene.into()
            }),
        )
        .frames([60.0]),
        Case::new(
            "count 0 draws nothing",
            rising(json!({ "count": 0 })),
            Expect::Uniform(CLEAR),
        ),
    ]);
}

/// A spray of `count` small boxes from the origin, falling back.
fn fountain(seed: i64, count: u32) -> Composition {
    render_3d(
        vec![
            shape(1, json!({ "size": [0.08, 0.08, 0.08] })),
            particles(
                2,
                None,
                1,
                json!({ "count": count, "seed": seed, "rate": 200, "lifetime": 1.5, "speed": 3,
                    "spread": 25, "gravity": [0, -4, 0], "scale": [0.6, 1.2],
                    "rotation_jitter": 180 }),
            ),
        ],
        &[2],
        front(),
    )
    .into()
}

#[test]
fn a_frame_renders_the_same_alone_as_after_the_frames_before_it() {
    let gpu = gpu();
    let media = TestMedia::default();
    let scene = fountain(7, 300);
    let mut in_order = gpu.renderer();
    for frame in 0..30 {
        render(&mut in_order, &scene, f64::from(frame), &media);
    }
    let after = render(&mut in_order, &scene, 30.0, &media);
    let alone = render(&mut gpu.renderer(), &fountain(7, 300), 30.0, &media);
    assert!(after.rgba.iter().any(|&byte| byte > 0), "frame 30 is blank");
    assert!(after.rgba == alone.rgba, "frame 30 differs alone");
    let earlier = render(&mut gpu.renderer(), &scene, 10.0, &media);
    assert!(earlier.rgba != alone.rgba, "particles move");
    let reseeded = render(&mut gpu.renderer(), &fountain(8, 300), 30.0, &media);
    assert!(
        reseeded.rgba != alone.rgba,
        "another seed scatters them elsewhere"
    );
}

/// `count` small boxes scattered over the node `surface` (id 1, with the nodes it needs after it),
/// seen by `camera`, with the surface left out.
fn scattered(count: u32, mut surface: Vec<Value>, camera: Value) -> crate::support::Scene {
    surface.extend([
        json!({ "id": 3, "type": "material_3d", "params": { "unlit": true } }),
        json!({ "id": 4, "type": "shape_3d", "params": { "size": [0.15, 0.15, 0.15] },
            "in": { "material": 3 } }),
        particles(2, Some(1), 4, json!({ "count": count, "seed": 3 })),
    ]);
    render_3d(surface, &[2], camera)
}

/// `count` small boxes scattered on a 4 × 4 plane, seen from above, with the plane left out.
fn scatter(count: u32) -> crate::support::Scene {
    scattered(
        count,
        vec![shape(1, json!({ "shape": "plane", "size": [4, 1, 4] }))],
        above(),
    )
}

/// Looks along -Z at the origin, 8 units across: 12 pixels to a unit.
fn facing() -> Value {
    json!({ "projection": "orthographic", "ortho_height": 8, "position": [0, 0, 10] })
}

/// The pixels of `frame` with any coverage, as `(x, y)`.
fn covered(frame: &Frame) -> Vec<(u32, u32)> {
    (0..frame.height)
        .flat_map(|y| (0..frame.width).map(move |x| (x, y)))
        .filter(|&(x, y)| frame.pixel(x, y)[3] > 0)
        .collect()
}

#[test]
fn a_static_scatter_lands_on_the_plane_and_nowhere_else() {
    check(vec![Case::new(
        "400 boxes over the middle half of the view",
        scatter(400),
        Expect::Custom(Box::new(|frames| {
            let frame = &frames[0];
            let (mut low, mut high, mut opaque) = (u32::MAX, 0, 0);
            for y in 0..frame.height {
                for x in 0..frame.width {
                    let alpha = frame.pixel(x, y)[3];
                    if alpha == 0 {
                        continue;
                    }
                    // The plane spans pixels 24 to 72, and a box sticks out of it by a pixel.
                    assert!(
                        (22..=74).contains(&x) && (22..=74).contains(&y),
                        "({x}, {y}) is drawn, off the plane"
                    );
                    low = low.min(x.min(y));
                    high = high.max(x.max(y));
                    opaque += u32::from(alpha == 255);
                }
            }
            assert!(opaque > 250, "only {opaque} pixels are covered");
            assert!(
                low < 28 && high > 68,
                "the boxes cover {low} to {high}, not the whole plane"
            );
        })),
    )]);
}

/// Thin posts scattered over a plane turned about Z by `tilt`, standing up along its normal when
/// `aligned`; or with `turned`, over a flat plane in a system turned by the tilt instead.
fn posts(tilt: f64, aligned: bool, turned: bool) -> Composition {
    let (plane, system) = if turned { (0.0, tilt) } else { (tilt, 0.0) };
    render_3d(
        vec![
            shape(
                1,
                json!({ "shape": "plane", "size": [3, 1, 3], "rotation": [0, 0, plane] }),
            ),
            shape(
                3,
                json!({ "size": [0.04, 0.6, 0.04], "position": [0, 0.3, 0] }),
            ),
            particles(
                2,
                Some(1),
                3,
                json!({ "count": 60, "seed": 5, "align_to_normal": aligned,
                    "rotation": [0, 0, system] }),
            ),
        ],
        &[2],
        front(),
    )
    .into()
}

/// The share of pixels that differ by more than a little between `a` and `b`.
fn differing(a: &Frame, b: &Frame) -> f64 {
    let pixels = a.rgba.chunks(4).zip(b.rgba.chunks(4));
    let differ = pixels
        .filter(|(a, b)| a.iter().zip(*b).any(|(a, b)| a.abs_diff(*b) > 8))
        .count();
    differ as f64 / (a.rgba.len() / 4) as f64
}

#[test]
fn aligned_particles_tilt_with_the_surface() {
    let gpu = gpu();
    let media = TestMedia::default();
    let frame = |scene: &Composition| render(&mut gpu.renderer(), scene, 0.0, &media);
    let tilted = frame(&posts(40.0, true, false));
    assert!(tilted.rgba.iter().any(|&byte| byte > 0), "nothing is drawn");
    // Posts standing on a tilted plane are posts on a flat one, all tilted together.
    let turned = frame(&posts(40.0, true, true));
    assert!(
        differing(&tilted, &turned) < 0.01,
        "{:.3} of pixels differ",
        differing(&tilted, &turned)
    );
    let upright = frame(&posts(40.0, false, false));
    assert!(
        differing(&tilted, &upright) > 0.02,
        "posts that don't follow the normal stand the same as those that do"
    );
}

#[test]
fn bind_time_is_flat_in_the_count() {
    let gpu = gpu();
    let media = TestMedia::default();
    let cost = |count: u32| {
        let scene: Composition = scatter(count).into();
        let mut renderer = gpu.renderer();
        let mut costs = Vec::new();
        for frame in [0.0, 1.0] {
            render(&mut renderer, &scene, frame, &media);
            let stats = renderer.frame_graph_stats().recorder;
            costs.push((
                renderer.recorded_pass_count(),
                stats.storage_bytes,
                stats.uniform_bytes,
            ));
        }
        costs
    };
    assert_eq!(cost(1000), cost(100_000));
    let moving = |count: u32| {
        let scene = fountain(1, count);
        let mut renderer = gpu.renderer();
        render(&mut renderer, &scene, 12.0, &media);
        (
            renderer.recorded_pass_count(),
            renderer.frame_graph_stats().recorder.storage_bytes,
        )
    };
    assert_eq!(moving(1000), moving(100_000));
}

fn render_error(scene: crate::support::Scene) -> String {
    let composition: Composition = scene.into();
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
fn particles_refuse_what_they_cant_draw_at_once() {
    let blended = render_error(render_3d(
        vec![
            json!({ "id": 3, "type": "material_3d", "params": { "opacity": 0.5 } }),
            json!({ "id": 1, "type": "shape_3d", "in": { "material": 3 } }),
            particles(2, None, 1, json!({})),
        ],
        &[2],
        front(),
    ));
    assert!(
        blended.contains("particles_3d") && blended.contains("a blended surface"),
        "{blended}"
    );
    let copied = render_error(render_3d(
        vec![
            shape(1, json!({})),
            particles(2, None, 1, json!({})),
            json!({ "id": 4, "type": "instance_3d", "params": { "count": 2 },
                "in": { "scene": 2 } }),
        ],
        &[4],
        front(),
    ));
    assert!(
        copied.contains("can't be copied more than once"),
        "{copied}"
    );
}

#[test]
fn a_surface_with_no_thickness_still_emits() {
    let frame = render(
        &mut gpu().renderer(),
        &scattered(
            400,
            vec![shape(1, json!({ "shape": "plane", "size": [4, 0, 4] }))],
            above(),
        )
        .into(),
        0.0,
        &TestMedia::default(),
    );
    let covered = covered(&frame);
    assert!(
        covered.len() > 250,
        "only {} pixels are covered",
        covered.len()
    );
    assert!(
        covered
            .iter()
            .all(|&(x, y)| (22..=74).contains(&x) && (22..=74).contains(&y))
    );
}

#[test]
fn an_extruded_solid_emits_over_its_faces() {
    // A 2 by 1 slab centred on the origin, 1 unit deep: 36 to 60 by 42 to 54 pixels.
    let frame = render(
        &mut gpu().renderer(),
        &scattered(
            400,
            vec![
                json!({ "id": 6, "type": "shape_2d",
                    "params": { "width": 200, "height": 100, "position": [-100, -50] } }),
                json!({ "id": 1, "type": "extrude_3d", "in": { "geometry": 6 } }),
            ],
            facing(),
        )
        .into(),
        0.0,
        &TestMedia::default(),
    );
    let covered = covered(&frame);
    assert!(
        covered.len() > 150,
        "only {} pixels are covered",
        covered.len()
    );
    assert!(
        covered
            .iter()
            .all(|&(x, y)| (34..=62).contains(&x) && (40..=56).contains(&y)),
        "boxes are drawn off the slab"
    );
}

#[test]
fn an_image_plane_emits_only_from_the_region_it_shows() {
    let pixels = (0..96 * 96).flat_map(|_| [255, 255, 255, 255]).collect();
    let media = TestMedia::default().image("white", 96, 96, pixels);
    let frame = render(
        &mut gpu().renderer(),
        &scattered(
            400,
            vec![
                json!({ "id": 5, "type": "media_in",
                    "params": { "kind": "image", "source": "white" } }),
                json!({ "id": 1, "type": "image_plane_3d",
                    "params": { "width": 4, "region": [0, 0, 48, 96] }, "in": { "source": 5 } }),
            ],
            facing(),
        )
        .into(),
        0.0,
        &media,
    );
    // The plane's left half is 24 to 48 pixels across, and a box sticks out of it by a pixel.
    let covered = covered(&frame);
    assert!(
        covered.len() > 150,
        "only {} pixels are covered",
        covered.len()
    );
    assert!(covered.iter().all(|&(x, _)| (22..=50).contains(&x)));
}
