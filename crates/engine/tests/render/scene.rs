//! `render_3d`: where the camera puts objects, what hides what, and when its output is kept.

use exr::prelude::WritableImage;
use lumen_3d::environment::Equirect;
use lumen_3d::glam::Vec3;
use lumen_engine::composition::Composition;
use serde_json::{Value, json};

use crate::support::{Case, Expect, Frame, Scene, TestMedia, check, gpu, render, scene};

pub(super) const SIZE: (u32, u32) = (64, 64);
pub(super) const CENTER: u32 = 32;
const CLEAR: [u8; 4] = [0, 0, 0, 0];
const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];

/// `scene_nodes`, the last of which is the scene, drawn by a `render_3d` filling the canvas.
fn render_3d(scene_nodes: Vec<Value>) -> Scene {
    render_3d_with(json!({}), scene_nodes)
}

pub(super) fn render_3d_with(params: Value, scene_nodes: Vec<Value>) -> Scene {
    let last = scene_nodes.last().expect("a scene node")["id"].clone();
    let mut nodes = scene_nodes;
    nodes
        .push(json!({ "id": 100, "type": "render_3d", "params": params, "in": { "scene": last } }));
    scene(SIZE, json!(nodes))
}

/// An unlit shape, which shows its colour exactly wherever it is drawn.
fn flat(id: u32, params: Value, color: [u8; 4]) -> Value {
    let mut params = params;
    params["color"] = json!(color);
    params["unlit"] = json!(true);
    json!({ "id": id, "type": "shape_3d", "params": params })
}

pub(super) fn merge(id: u32, objects: &[u32], params: Value) -> Value {
    json!({ "id": id, "type": "merge_3d", "params": params, "in": { "objects": objects } })
}

/// Asserts that the white box covers `(x, y)` of `frame`, its front lit by the default key light,
/// ambient fill and studio environment.
fn lit_at(frame: &Frame, x: u32, y: u32) {
    let [r, g, b, a] = frame.pixel(x, y);
    assert_eq!(a, 255, "the box covers ({x}, {y})");
    assert!(
        r.abs_diff(214) <= 8 && r == g && g == b,
        "a white box's front is lit light grey, got {r} {g} {b}"
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
fn overlapping_blended_objects_composite_from_back_to_front_in_either_node_order() {
    let near = flat(
        1,
        json!({ "size": [1, 1, 0.2], "position": [0.4, 0, 1], "opacity": 0.5 }),
        GREEN,
    );
    let far = flat(
        2,
        json!({ "size": [1, 1, 0.2], "position": [-0.4, 0, -1], "opacity": 0.5 }),
        RED,
    );
    // Half green over half red, in linear light: green 0.5, red 0.25, alpha 0.75. Red over green
    // would swap the red and green channels.
    let overlap = || Expect::Probes(vec![(CENTER, CENTER, [117, 160, 0, 191])]);
    check(vec![
        Case::new(
            "near node first",
            render_3d(vec![
                near.clone(),
                far.clone(),
                merge(3, &[1, 2], json!({})),
            ]),
            overlap(),
        )
        .tolerance(2),
        Case::new(
            "far node first",
            render_3d(vec![near, far, merge(3, &[2, 1], json!({}))]),
            overlap(),
        )
        .tolerance(2),
    ]);
}

/// A lit plane facing the default camera, with `params` on top of its own.
fn facing_plane(params: Value) -> Value {
    let mut plane = json!({ "shape": "plane", "rotation": [90, 0, 0] });
    for (name, value) in params.as_object().expect("params") {
        plane[name] = value.clone();
    }
    json!({ "id": 1, "type": "shape_3d", "params": plane })
}

/// Rows that `nodes`, under a `render_3d`, render the plain facing plane: pixel for pixel, and
/// as a lit plane on its own, so that two blank renders can't pass.
fn like_the_plain_plane(name: &str, nodes: Vec<Value>) -> [Case; 2] {
    [
        Case::new(
            format!("{name} matches the plain plane"),
            render_3d(nodes.clone()),
            Expect::Matches(render_3d(vec![facing_plane(json!({}))]).into()),
        ),
        Case::new(
            format!("{name} shows a lit plane"),
            render_3d(nodes),
            Expect::Custom(Box::new(|frames| {
                lit_at(&frames[0], CENTER, CENTER);
                // A unit plane at the origin is about 17.6 pixels across.
                let across = covered(&frames[0], CENTER, 0..SIZE.0);
                assert!((17..=19).contains(&across), "{across} pixels across");
            })),
        ),
    ]
}

#[test]
fn a_mirrored_object_shows_and_is_lit_as_its_unmirrored_twin() {
    check(
        [
            like_the_plain_plane(
                "single-sided",
                vec![facing_plane(json!({ "scale": [-1, 1, 1] }))],
            ),
            like_the_plain_plane(
                "double-sided",
                vec![facing_plane(
                    json!({ "scale": [-1, 1, 1], "double_sided": true }),
                )],
            ),
            like_the_plain_plane(
                "mirrored by its merge_3d",
                vec![
                    facing_plane(json!({})),
                    merge(2, &[1], json!({ "scale": [-1, 1, 1] })),
                ],
            ),
        ]
        .into_iter()
        .flatten()
        .collect(),
    );
}

#[test]
fn a_plane_without_thickness_is_lit_as_one_with_it() {
    // A plane has no extent across Y, so flattening Y leaves it as it was.
    check(
        [
            like_the_plain_plane(
                "zero size",
                vec![facing_plane(json!({ "size": [1, 0, 1] }))],
            ),
            like_the_plain_plane(
                "zero scale",
                vec![facing_plane(json!({ "scale": [1, 0, 1] }))],
            ),
        ]
        .into_iter()
        .flatten()
        .collect(),
    );
}

#[test]
fn a_plane_scaled_up_with_its_camera_is_lit_as_a_unit_one() {
    let scaled = |scale: f64| {
        vec![
            facing_plane(json!({ "size": [scale, 1, scale] })),
            json!({ "id": 2, "type": "camera_3d", "params": {
                "position": [0, 0, 5.0 * scale], "near": 0.05 * scale, "far": 1000.0 * scale
            } }),
            merge(3, &[1, 2], json!({})),
        ]
    };
    check(
        [
            like_the_plain_plane("1e10 times", scaled(1e10)),
            like_the_plain_plane("1e20 times", scaled(1e20)),
        ]
        .into_iter()
        .flatten()
        .collect(),
    );
}

#[test]
fn point_and_spot_lights_leave_what_they_do_not_reach_unlit() {
    // A wall filling the canvas, lit by `light` alone.
    let lit_by = |light: Value| {
        scene(
            SIZE,
            json!([
                facing_plane(json!({ "size": [4, 1, 4] })),
                { "id": 2, "type": "light_3d", "params": light },
                merge(3, &[1, 2], json!({})),
                { "id": 100, "type": "render_3d", "params": { "environment": 0 }, "in": { "scene": 3 } },
            ]),
        )
    };
    // One unit right of centre on the wall.
    let right = (49, CENTER);
    let unlit = [0, 0, 0, 255];
    let lit = |frame: &Frame, (x, y): (u32, u32)| {
        let [r, _, _, a] = frame.pixel(x, y);
        assert!(r > 100 && a == 255, "({x}, {y}) is lit, got {r}");
    };
    check(vec![
        Case::new(
            "a point light a unit left of centre reaches 1.5 units",
            lit_by(json!({ "light": "point", "position": [-1, 0, 1], "range": 1.5 })),
            Expect::Custom(Box::new(move |frames| {
                lit(&frames[0], (15, CENTER));
                assert_eq!(frames[0].pixel(right.0, right.1), unlit);
            })),
        ),
        Case::new(
            "a spot light 2 units out shines a 30° cone at the centre",
            lit_by(
                json!({ "light": "spot", "position": [0, 0, 2], "rotation": [0, 0, 0], "spot_angle": 30 }),
            ),
            Expect::Custom(Box::new(move |frames| {
                lit(&frames[0], (CENTER, CENTER));
                assert_eq!(frames[0].pixel(right.0, right.1), unlit);
            })),
        ),
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

fn render_error(composition: &Composition) -> String {
    gpu()
        .renderer()
        .render_frame_at(composition, 0.0, &TestMedia::default())
        .expect_err("the render fails")
        .to_string()
}

#[test]
fn a_scene_that_expands_past_the_object_limit_fails_before_expanding_it() {
    // Each merge_3d takes the one before twice, so the box is reached 2^40 times.
    let mut nodes = vec![json!({ "id": 1, "type": "shape_3d" })];
    for id in 2..=41 {
        nodes.push(merge(id, &[id - 1, id - 1], json!({})));
    }
    let error = render_error(&render_3d(nodes).into());
    assert!(
        error.contains(
            "the scene expands to more than 100000 nodes, counting a node each time it is reached"
        ),
        "{error}"
    );
}

#[test]
fn a_long_chain_of_merge_3d_renders() {
    // Ids from 1000, clear of the render_3d's.
    let mut nodes = vec![json!({ "id": 1000, "type": "shape_3d" })];
    for id in 1001..=6000 {
        nodes.push(merge(id, &[id - 1], json!({})));
    }
    check(vec![Case::new(
        "5000 merge_3d above a box",
        render_3d(nodes),
        Expect::Custom(Box::new(|frames| lit_at(&frames[0], CENTER, CENTER))),
    )]);
}

#[test]
fn an_output_larger_than_the_device_allows_fails() {
    let max = gpu()
        .renderer()
        .gpu_renderer()
        .device
        .limits()
        .max_texture_dimension_2d;
    for width in [max + 1, u32::MAX] {
        let composition = scene(
            SIZE,
            json!([
                { "id": 1, "type": "shape_3d" },
                { "id": 100, "type": "render_3d", "params": { "width": width, "height": 1 }, "in": { "scene": 1 } },
            ]),
        )
        .into();
        let error = render_error(&composition);
        assert!(
            error.contains(&format!(
                "a {width}x1 output is larger than this device's {max}-pixel texture limit"
            )),
            "{error}"
        );
    }
}

const BLUE: [u8; 4] = [0, 0, 255, 255];

/// A 64 x 64 image of straight RGBA, the colour of each pixel given by `color(x, y)`.
fn image(color: impl Fn(u32, u32) -> [u8; 4]) -> TestMedia {
    let pixels = (0..SIZE.1)
        .flat_map(|y| (0..SIZE.0).map(move |x| (x, y)))
        .flat_map(|(x, y)| color(x, y))
        .collect();
    TestMedia::default().image("plate", SIZE.0, SIZE.1, pixels)
}

/// A 64 x 64 image with red growing to the right and green downwards.
fn gradient() -> TestMedia {
    image(|x, y| [x as u8 * 4, y as u8 * 4, 128, 255])
}

/// The image `plate` as a 2D raster.
fn plate(id: u32) -> Value {
    json!({ "id": id, "type": "media_in", "params": { "kind": "image", "source": "plate" } })
}

fn plane(id: u32, source: u32, params: Value) -> Value {
    json!({ "id": id, "type": "image_plane_3d", "params": params, "in": { "source": source } })
}

/// A camera that sees 4 units across the canvas however far away, so that a plane 4 units wide
/// fills the frame and maps its texels to pixels.
pub(super) fn ortho(id: u32, params: Value) -> Value {
    let mut camera = json!({ "projection": "orthographic", "ortho_height": 4 });
    for (name, value) in params.as_object().expect("params") {
        camera[name] = value.clone();
    }
    json!({ "id": id, "type": "camera_3d", "params": camera })
}

/// A plane filling the frame, in front of an orthographic camera.
fn framed(source: u32) -> Vec<Value> {
    vec![
        plane(1, source, json!({ "width": 4 })),
        ortho(2, json!({})),
        merge(3, &[1, 2], json!({})),
    ]
}

#[test]
fn an_unlit_plane_filling_the_frame_reproduces_its_source_whatever_the_tone_mapping() {
    let source = || scene(SIZE, json!([plate(1)]));
    let seen = |params: Value| {
        let mut nodes = vec![plate(10)];
        nodes.extend(framed(10));
        render_3d_with(params, nodes)
    };
    // One texel to a pixel, sampled at texel centres.
    let exact = |name: &str, params: Value| {
        Case::new(name, seen(params), Expect::Matches(source().into()))
            .media(gradient())
            .tolerance(1)
    };
    check(vec![
        exact("untouched", json!({ "supersample": 1 })),
        exact(
            "with ACES and an extra stop",
            json!({ "supersample": 1, "tone_mapping": "aces", "exposure": 1 }),
        ),
        exact(
            "with AgX and a stop less",
            json!({ "supersample": 1, "tone_mapping": "agx", "exposure": -1 }),
        ),
        exact(
            "with the default supersampling and ACES",
            json!({ "tone_mapping": "aces", "exposure": 1 }),
        ),
    ]);
}

/// A checkerboard of one-pixel squares on the left, and on the right one-pixel white strokes
/// across black.
fn fine_detail() -> TestMedia {
    image(|x, y| {
        let white = if x < CENTER {
            (x + y) % 2 == 0
        } else {
            x == 48 || y == 20
        };
        if white { [255; 4] } else { [0, 0, 0, 255] }
    })
}

#[test]
fn a_plane_drawn_pixel_for_pixel_keeps_one_pixel_detail_at_any_supersample() {
    let seen = |supersample: u32| {
        let mut nodes = vec![plate(10)];
        nodes.extend(framed(10));
        render_3d_with(json!({ "supersample": supersample }), nodes)
    };
    check(
        [1, 2, 3, 4]
            .map(|supersample| {
                Case::new(
                    format!("supersample {supersample}"),
                    seen(supersample),
                    Expect::Matches(scene(SIZE, json!([plate(1)])).into()),
                )
                .media(fine_detail())
            })
            .into(),
    );
}

#[test]
fn a_plane_smaller_than_its_source_is_filtered_by_supersampling_alone() {
    // 32 x 32 output pixels show 64 x 64 texels. Without supersampling a pixel reads the raster at
    // its centre, so moving the plane a quarter pixel each way reads it where the four
    // supersamples of the default do; averaging those four in linear light is what it should give.
    let seen = |params: Value, offset: [f64; 2]| {
        let mut nodes = vec![plate(10)];
        nodes.extend([
            plane(
                1,
                10,
                json!({ "width": 2, "position": [offset[0], offset[1], 0] }),
            ),
            ortho(2, json!({})),
            merge(3, &[1, 2], json!({})),
        ]);
        Composition::from(render_3d_with(params, nodes))
    };
    let to_linear = |byte: u8| {
        let c = f64::from(byte) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let to_srgb = |c: f64| {
        let c = if c <= 0.0031308 {
            c * 12.92
        } else {
            1.055 * c.powf(1.0 / 2.4) - 0.055
        };
        (c * 255.0).round() as u8
    };
    let gpu = gpu();
    let media = fine_detail();
    let supersampled = render(
        &mut gpu.renderer(),
        &seen(json!({ "supersample": 2 }), [0.0, 0.0]),
        0.0,
        &media,
    );
    // A quarter pixel is 1/64 of a unit.
    let quarter = 1.0 / 64.0;
    let shifted: Vec<Frame> = [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)]
        .iter()
        .map(|(x, y)| {
            let composition = seen(json!({ "supersample": 1 }), [x * quarter, y * quarter]);
            render(&mut gpu.renderer(), &composition, 0.0, &media)
        })
        .collect();
    // The plane covers pixels 16 to 48, and its edges are left out.
    for y in 18..46 {
        for x in 18..46 {
            for channel in 0..3 {
                let sum: f64 = shifted
                    .iter()
                    .map(|frame| to_linear(frame.pixel(x, y)[channel]))
                    .sum();
                let expected = to_srgb(sum / 4.0);
                let actual = supersampled.pixel(x, y)[channel];
                assert!(
                    actual.abs_diff(expected) <= 1,
                    "({x}, {y}) channel {channel} is {actual}, expected {expected}"
                );
            }
        }
    }
}

#[test]
fn a_planes_silhouette_is_antialiased_without_a_seam_at_the_default_supersample() {
    // A blue square turned 20° and not filling the frame has partly covered edge pixels, each
    // blue scaled by its coverage like any other blended edge.
    let mut nodes = vec![plate(10)];
    nodes.extend([
        plane(1, 10, json!({ "width": 2, "rotation": [0, 0, 20] })),
        ortho(2, json!({})),
        merge(3, &[1, 2], json!({})),
    ]);
    check(vec![
        Case::new(
            "a rotated plane",
            render_3d(nodes),
            Expect::Custom(Box::new(|frames| {
                let edges: Vec<[u8; 4]> = frames[0]
                    .rgba
                    .chunks(4)
                    .map(|pixel| [pixel[0], pixel[1], pixel[2], pixel[3]])
                    .filter(|pixel| (1..255).contains(&pixel[3]))
                    .collect();
                assert!(edges.len() > 40, "{} edge pixels", edges.len());
                for edge in edges {
                    assert!(
                        edge[0] == 0 && edge[1] == 0 && edge[2].abs_diff(edge[3]) <= 2,
                        "an edge pixel is blue scaled by coverage, got {edge:?}"
                    );
                }
                assert_eq!(frames[0].pixel(CENTER, CENTER), BLUE);
            })),
        )
        .media(image(|_, _| BLUE)),
    ]);
}

#[test]
fn opacity_scales_the_alpha_of_a_source() {
    let mut nodes = vec![plate(10)];
    nodes.extend([
        plane(1, 10, json!({ "width": 4, "opacity": 0.5 })),
        ortho(2, json!({})),
        merge(3, &[1, 2], json!({})),
    ]);
    check(vec![
        Case::new(
            "half-transparent blue at half opacity",
            render_3d(nodes),
            Expect::Probes(vec![(CENTER, CENTER, [0, 0, 64, 64])]),
        )
        .media(image(|_, _| [0, 0, 255, 128])),
    ]);
}

/// A plane 4 units wide showing the clip of a 128 x 64 composition whose output `output`, which
/// is 64 x 32 pixels of the 64 x 64 frame.
fn wide_clip(output: Value) -> Scene {
    let mut wide = render_3d(framed(10));
    wide.0["compositions"] = json!({ "wide": {
        "render_settings": { "width": 128, "height": 64 },
        "timeline": { "fps": 30, "duration_frames": 1 },
        "nodes": [output, { "id": 2, "type": "media_output" }],
        "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }],
    } });
    wide.0["nodes"].as_array_mut().expect("nodes").push(json!({
        "id": 10, "type": "clip", "params": { "composition": "wide" }
    }));
    wide
}

#[test]
fn a_planes_height_follows_the_canvas_of_its_source() {
    let background = json!({ "id": 1, "type": "background", "params": { "paint": RED } });
    check(vec![Case::new(
        "a clip whose canvas is twice as wide as tall",
        wide_clip(background),
        Expect::Custom(Box::new(|frames| {
            let frame = &frames[0];
            let tall = (0..SIZE.1)
                .filter(|&y| frame.pixel(CENTER, y)[3] == 255)
                .count();
            assert_eq!(tall, 32);
            assert_eq!(covered(frame, CENTER, 0..SIZE.0), 64);
            assert_eq!(frame.pixel(CENTER, 20), RED);
            assert_eq!(frame.pixel(CENTER, 10), CLEAR);
        })),
    )]);
}

#[test]
fn a_clip_content_is_padded_or_cropped_to_the_canvas_it_declares() {
    let image_out =
        json!({ "id": 1, "type": "media_in", "params": { "kind": "image", "source": "plate" } });
    let image_of = |width: u32, color: fn(u32) -> [u8; 4]| {
        let pixels = (0..64)
            .flat_map(|_| (0..width).flat_map(color))
            .collect::<Vec<u8>>();
        TestMedia::default().image("plate", width, 64, pixels)
    };
    check(vec![
        // Half the declared 128 pixels across, so the right half of the plane is padding.
        Case::new(
            "a 64 x 64 image on a 128 x 64 canvas",
            wide_clip(image_out.clone()),
            Expect::Custom(Box::new(|frames| {
                let frame = &frames[0];
                for (x, y, color) in [
                    (16, 20, RED),
                    (16, 44, RED),
                    (16, 10, CLEAR),
                    (16, 54, CLEAR),
                    (48, 32, CLEAR),
                ] {
                    assert_eq!(frame.pixel(x, y), color, "({x}, {y})");
                }
            })),
        )
        .media(image_of(64, |_| RED)),
        // Red for the declared 128 pixels, then green beyond them.
        Case::new(
            "a 256 x 64 image on a 128 x 64 canvas",
            wide_clip(image_out),
            Expect::Custom(Box::new(|frames| {
                let frame = &frames[0];
                for (x, y, color) in [(8, 20, RED), (56, 20, RED), (56, 44, RED), (56, 10, CLEAR)] {
                    assert_eq!(frame.pixel(x, y), color, "({x}, {y})");
                }
            })),
        )
        .media(image_of(256, |x| if x < 128 { RED } else { GREEN })),
    ]);
}

/// Opaque blue on the left half, nothing on the right.
fn half_blue() -> TestMedia {
    image(|x, _| if x < CENTER { BLUE } else { CLEAR })
}

#[test]
fn a_planes_transparent_texels_show_what_is_behind_without_hiding_it() {
    let behind = |plane_first: bool| {
        let objects: Vec<Value> = vec![
            plate(10),
            plane(1, 10, json!({ "width": 4 })),
            ortho(2, json!({})),
            flat(
                4,
                json!({ "size": [4, 4, 0.2], "position": [0, 0, -2] }),
                RED,
            ),
            flat(
                5,
                json!({ "size": [1, 1, 0.2], "position": [1, 0, -1], "opacity": 0.5 }),
                GREEN,
            ),
        ];
        let order: &[u32] = if plane_first {
            &[1, 2, 4, 5]
        } else {
            &[4, 5, 2, 1]
        };
        let mut nodes = objects;
        nodes.push(merge(6, order, json!({})));
        render_3d(nodes)
    };
    // Left of centre the plane covers everything. Right of it the red wall shows, with a
    // half-transparent green box in front of it, and the plane in front of both.
    let expect = || {
        Expect::Probes(vec![
            (16, CENTER, BLUE),
            (48, CENTER, [188, 188, 0, 255]),
            (60, CENTER, RED),
        ])
    };
    check(vec![
        Case::new("the plane is listed first", behind(true), expect())
            .media(half_blue())
            .tolerance(2),
        Case::new("the plane is listed last", behind(false), expect())
            .media(half_blue())
            .tolerance(2),
        // Where a texel is partly transparent its colour is not darkened by its neighbours.
        Case::new(
            "an edge that is half covered keeps its colour",
            render_3d_with(json!({}), {
                // Half a pixel to the right puts the source's edge in the middle of pixel 32.
                let mut nodes = vec![plate(10)];
                nodes.extend([
                    plane(1, 10, json!({ "width": 4, "position": [0.03125, 0, 0] })),
                    ortho(2, json!({})),
                    merge(3, &[1, 2], json!({})),
                ]);
                nodes
            }),
            Expect::Custom(Box::new(|frames| {
                let frame = &frames[0];
                let edge = frame.pixel(CENTER, CENTER);
                assert!(
                    (1..255).contains(&edge[3]),
                    "the edge is partly covered, got {edge:?}"
                );
                assert!(
                    edge[0] == 0 && edge[1] == 0 && edge[2].abs_diff(edge[3]) <= 2,
                    "partly covered blue is blue scaled by its coverage, got {edge:?}"
                );
            })),
        )
        .media(half_blue()),
    ]);
}

/// A blue plane turned 45° about Y, so that it passes through a half-transparent green box
/// centred nearer the camera than the plane's centre: right of centre the plane is behind the box,
/// left of centre in front of it.
fn plane_through_a_box(plane_params: Value) -> Scene {
    let mut params = json!({ "width": 4, "rotation": [0, 45, 0] });
    for (name, value) in plane_params.as_object().expect("params") {
        params[name] = value.clone();
    }
    render_3d(vec![
        plate(10),
        plane(1, 10, params),
        ortho(2, json!({})),
        flat(
            3,
            json!({ "size": [3, 1, 0.1], "position": [0, 0, 0.5], "opacity": 0.5 }),
            GREEN,
        ),
        merge(4, &[1, 2, 3], json!({})),
    ])
}

#[test]
fn an_opaque_plane_is_ordered_by_depth_against_the_blended_objects_it_crosses() {
    // Half green over blue in linear light is sRGB 188 in both channels.
    let box_over_plane = [0, 188, 188, 255];
    check(vec![
        // A blended plane is ordered by its centre, which is behind the box's, so the box
        // blends over it wherever they overlap, including left of centre where the plane is in
        // front.
        Case::new(
            "a blended plane is overdrawn by the box on both sides",
            plane_through_a_box(json!({})),
            Expect::Probes(vec![
                (16, CENTER, box_over_plane),
                (48, CENTER, box_over_plane),
            ]),
        )
        .media(image(|_, _| BLUE))
        .tolerance(2),
        Case::new(
            "an opaque plane hides the box where it is in front and shows through it where behind",
            plane_through_a_box(json!({ "opaque": true })),
            Expect::Probes(vec![(16, CENTER, BLUE), (48, CENTER, box_over_plane)]),
        )
        .media(image(|_, _| BLUE))
        .tolerance(2),
        Case::new(
            "opaque false is the default",
            plane_through_a_box(json!({ "opaque": false })),
            Expect::Probes(vec![
                (16, CENTER, box_over_plane),
                (48, CENTER, box_over_plane),
            ]),
        )
        .media(image(|_, _| BLUE))
        .tolerance(2),
    ]);
}

#[test]
fn an_opaque_plane_shows_transparent_texels_in_their_colour_at_full_opacity() {
    let framed_plane = |params: Value| {
        let mut params = params;
        params["width"] = json!(4);
        render_3d(vec![
            plate(10),
            plane(1, 10, params),
            ortho(2, json!({})),
            merge(3, &[1, 2], json!({})),
        ])
    };
    check(vec![
        Case::new(
            "blended, the transparent half shows nothing",
            framed_plane(json!({})),
            Expect::Probes(vec![(16, CENTER, BLUE), (48, CENTER, CLEAR)]),
        )
        .media(half_blue()),
        Case::new(
            "opaque, the transparent half is opaque black and opacity is ignored",
            framed_plane(json!({ "opaque": true, "opacity": 0.2 })),
            Expect::Probes(vec![(16, CENTER, BLUE), (48, CENTER, [0, 0, 0, 255])]),
        )
        .media(half_blue()),
    ]);
}

#[test]
fn a_lit_plane_is_coloured_by_its_source_and_responds_to_light() {
    // Red on the left and white on the right, lit head-on or from behind.
    let lit_by = |rotation: [i32; 3]| {
        let mut nodes = vec![plate(10)];
        nodes.extend([
            plane(1, 10, json!({ "width": 4, "unlit": false, "roughness": 1 })),
            ortho(2, json!({})),
            json!({ "id": 3, "type": "light_3d", "params": { "rotation": rotation } }),
            merge(4, &[1, 2, 3], json!({})),
        ]);
        render_3d_with(json!({ "environment": 0 }), nodes)
    };
    let halves = || image(|x, _| if x < CENTER { RED } else { [255; 4] });
    check(vec![
        Case::new(
            "a light shining on its face",
            lit_by([0, 0, 0]),
            Expect::Custom(Box::new(|frames| {
                let [r, g, _, a] = frames[0].pixel(16, CENTER);
                assert!(
                    a == 255 && r > 150 && g < 40,
                    "red under light, got {r} {g}"
                );
                let [r, g, b, _] = frames[0].pixel(48, CENTER);
                assert!(r > 150 && r.abs_diff(g) <= 2 && g.abs_diff(b) <= 2);
            })),
        )
        .media(halves()),
        Case::new(
            "a light shining on its back",
            lit_by([0, 180, 0]),
            Expect::Probes(vec![
                (16, CENTER, [0, 0, 0, 255]),
                (48, CENTER, [0, 0, 0, 255]),
            ]),
        )
        .media(halves()),
    ]);
}

#[test]
fn a_plane_seen_from_behind_is_visible_and_mirrored() {
    let from = |z: f64| {
        let mut nodes = vec![plate(10)];
        nodes.extend([
            plane(1, 10, json!({ "width": 4 })),
            ortho(2, json!({ "position": [0, 0, z], "look_at": true })),
            merge(3, &[1, 2], json!({})),
        ]);
        render_3d(nodes)
    };
    let halves = || image(|x, _| if x < CENTER { RED } else { GREEN });
    check(vec![
        Case::new(
            "from the front",
            from(5.0),
            Expect::Probes(vec![(16, CENTER, RED), (48, CENTER, GREEN)]),
        )
        .media(halves()),
        Case::new(
            "from behind",
            from(-5.0),
            Expect::Probes(vec![(16, CENTER, GREEN), (48, CENTER, RED)]),
        )
        .media(halves()),
    ]);
}

#[test]
fn a_render_3d_over_a_plane_is_kept_until_the_source_animates() {
    let orbit = |paint: &str| {
        let mut composition = render_3d(vec![
            json!({ "id": 10, "type": "background", "params": { "paint": paint } }),
            plane(1, 10, json!({ "width": 3 })),
        ])
        .frames(4);
        composition.0["timeline"]["fps"] = json!(4);
        Composition::from(composition)
    };
    let gpu = gpu();
    let media = TestMedia::default();
    let frames = |composition: &Composition| {
        let mut renderer = gpu.renderer();
        let frames: Vec<Vec<u8>> = (0..4)
            .map(|frame| render(&mut renderer, composition, f64::from(frame), &media).rgba)
            .collect();
        (frames, renderer.skipped_pass_count())
    };

    let (still, skipped) = frames(&orbit("#3366cc"));
    assert!(still.iter().all(|frame| *frame == still[0]));
    assert!(
        skipped >= 3 * 2,
        "frames after the first read the kept draw and resolve, skipping {skipped} passes"
    );

    let (moving, skipped) = frames(&orbit("=rgb(60 * frame, 100, 200)"));
    assert_eq!(skipped, 0, "a source that animates renders every frame");
    for pair in moving.windows(2) {
        assert!(pair[0] != pair[1], "the plane's colour changes");
    }
}

#[test]
fn a_planes_source_keeps_its_place_within_the_canvas_when_it_draws_less_than_all_of_it() {
    // A 2D shape that fills a corner of the canvas is a bounded raster. The plane shows the whole
    // canvas, so the shape stays in that corner of the plane, at the plane's scale.
    let mut nodes = vec![json!({ "id": 10, "type": "shape", "params": {
        "position": [8, 8], "width": 16, "height": 16, "fill_paint": GREEN
    } })];
    nodes.extend(framed(10));
    check(vec![Case::new(
        "a 16 pixel square near the top left",
        render_3d(nodes),
        Expect::Custom(Box::new(|frames| {
            for (x, y, color) in [
                (16, 16, GREEN),
                (40, 16, CLEAR),
                (16, 40, CLEAR),
                (56, 56, CLEAR),
            ] {
                assert_eq!(frames[0].pixel(x, y), color, "({x}, {y})");
            }
        })),
    )]);
}

#[test]
fn compiling_reaches_the_programs_of_a_planes_source_through_the_scene_nodes() {
    let mut nodes = vec![
        json!({ "id": 10, "type": "shape", "params": { "width": 16, "height": 16 } }),
        json!({ "id": 11, "type": "blur", "params": { "radius": 2 }, "in": { "source": 10 } }),
        plane(1, 11, json!({ "width": 4 })),
        // Shadows, and a catcher from the second frame on, that only an expression turns on.
        json!({ "id": 3, "type": "shape_3d", "params": { "size": [0.5, 0.5, 0.5], "position": [0, 0, 1] } }),
        json!({ "id": 4, "type": "shape_3d", "params": {
            "shape": "plane", "size": [6, 1, 6], "position": [0, -1, 0], "shadow_catcher": "=frame > 0" } }),
        light(
            5,
            json!({ "light": "directional", "rotation": [-60, 0, 0], "cast_shadows": true }),
        ),
        merge(2, &[1, 3, 4, 5], json!({})),
    ];
    let composition = Composition::from(render_3d(std::mem::take(&mut nodes)).frames(2));
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    renderer.compile(&composition).expect("the scene compiles");
    let compiled = renderer.created_program_count();
    assert!(compiled > 0);

    for frame in [0.0, 1.0] {
        render(&mut renderer, &composition, frame, &TestMedia::default());
    }

    assert_eq!(renderer.created_program_count(), compiled);

    // A scene that shows its environment map behind it.
    let media = TestMedia::default().environment("sky.hdr", environment_file(16, 8, |_| Vec3::ONE));
    let composition = Composition::from(render_3d_with(
        lit_by("sky.hdr", json!({ "environment_background": true })),
        vec![sphere(json!({}))],
    ));
    let mut renderer = gpu.renderer();
    renderer.compile(&composition).expect("the scene compiles");
    let compiled = renderer.created_program_count();
    render(&mut renderer, &composition, 0.0, &media);
    assert_eq!(renderer.created_program_count(), compiled);
}

#[test]
fn a_clip_canvas_larger_than_the_device_allows_fails_naming_the_clip() {
    let max = gpu()
        .renderer()
        .gpu_renderer()
        .device
        .limits()
        .max_texture_dimension_2d;
    let width = max + 1;
    let mut wide = wide_clip(
        json!({ "id": 1, "type": "media_in", "params": { "kind": "image", "source": "plate" } }),
    );
    wide.0["compositions"]["wide"]["render_settings"]["width"] = json!(width);
    let error = gpu()
        .renderer()
        .render_frame_at(&wide.into(), 0.0, &image(|_, _| RED))
        .expect_err("the render fails")
        .to_string();
    assert!(
        error.contains(&format!(
            "a {width}x64 canvas is larger than this device's {max}-pixel texture limit"
        )) && error.contains("Clip"),
        "{error}"
    );
}

/// The glTF sample fox: skinned, textured, without vertex normals, with the clips `Survey`,
/// `Walk` and `Run`.
fn fox_media() -> TestMedia {
    TestMedia::default().model("fox.glb", include_bytes!("../fixtures/Fox.glb").to_vec())
}

/// The fox standing side on to the default camera, nose to the right, filling most of the frame.
fn fox(id: u32, params: Value) -> Value {
    let mut fox = json!({
        "source": "fox.glb",
        "position": [0, -0.8, 0],
        "rotation": [0, 90, 0],
        "scale": [0.02, 0.02, 0.02],
    });
    for (name, value) in params.as_object().expect("params") {
        fox[name] = value.clone();
    }
    json!({ "id": id, "type": "model_3d", "params": fox })
}

/// A `.glb` of one mesh with a primitive for each of `quads`: a rectangle facing +Z, given by its
/// left, bottom, right and top edges, that shows the whole of its textures, top left at top left,
/// in the material its index in `materials` names.
fn quads(materials: Value, quads: &[([f32; 4], usize)]) -> Vec<u8> {
    let quads: Vec<Quad> = quads
        .iter()
        .map(|&(edges, material)| Quad {
            edges,
            material,
            normal: [0.0, 0.0, 1.0],
            mirrored_u: false,
        })
        .collect();
    model_glb(materials, None, &quads)
}

/// A rectangle facing +Z for [`model_glb`].
struct Quad {
    /// Its left, bottom, right and top edges.
    edges: [f32; 4],
    material: usize,
    /// The normal at each of its vertices.
    normal: [f32; 3],
    /// Whether its texture coordinates run right to left.
    mirrored_u: bool,
}

/// A `.glb` of one mesh with a primitive for each of `quads`, which show the whole of their
/// textures, top left at top left. `image`, a PNG, is texture 0.
fn model_glb(materials: Value, image: Option<&[u8]>, quads: &[Quad]) -> Vec<u8> {
    let mut bin = Vec::new();
    let (mut views, mut accessors, mut primitives) = (Vec::new(), Vec::new(), Vec::new());
    let mut accessor = |bytes: Vec<u8>, kind: &str, component: u32, count: usize| {
        views.push(json!({ "buffer": 0, "byteOffset": bin.len(), "byteLength": bytes.len() }));
        bin.extend(bytes);
        bin.resize(bin.len().next_multiple_of(4), 0);
        accessors.push(json!({
            "bufferView": views.len() - 1, "componentType": component, "type": kind, "count": count
        }));
        accessors.len() - 1
    };
    let floats =
        |values: &[f32]| -> Vec<u8> { values.iter().flat_map(|v| v.to_le_bytes()).collect() };
    for quad in quads {
        let [left, bottom, right, top] = quad.edges;
        let positions = [
            left, top, 0.0, right, top, 0.0, left, bottom, 0.0, right, bottom, 0.0,
        ];
        let position = accessor(floats(&positions), "VEC3", 5126, 4);
        let normal = accessor(floats(&quad.normal.repeat(4)), "VEC3", 5126, 4);
        let (start, end) = if quad.mirrored_u {
            (1.0, 0.0)
        } else {
            (0.0, 1.0)
        };
        let uv = accessor(
            floats(&[start, 0.0, end, 0.0, start, 1.0, end, 1.0]),
            "VEC2",
            5126,
            4,
        );
        let indices: Vec<u8> = [0u16, 2, 1, 1, 2, 3]
            .iter()
            .flat_map(|i| i.to_le_bytes())
            .collect();
        let indices = accessor(indices, "SCALAR", 5123, 6);
        primitives.push(json!({
            "attributes": { "POSITION": position, "NORMAL": normal, "TEXCOORD_0": uv },
            "indices": indices,
            "material": quad.material,
        }));
    }
    let image = image.map(|image| {
        views.push(json!({ "buffer": 0, "byteOffset": bin.len(), "byteLength": image.len() }));
        bin.extend(image);
        bin.resize(bin.len().next_multiple_of(4), 0);
        views.len() - 1
    });
    let mut document = json!({
        "asset": { "version": "2.0" },
        "extensionsUsed": ["KHR_materials_unlit"],
        "scene": 0,
        "scenes": [{ "nodes": [0] }],
        "nodes": [{ "mesh": 0 }],
        "meshes": [{ "primitives": primitives }],
        "materials": materials,
        "accessors": accessors,
        "bufferViews": views,
        "buffers": [{ "byteLength": bin.len() }],
    });
    if let Some(view) = image {
        document["images"] = json!([{ "bufferView": view, "mimeType": "image/png" }]);
        document["textures"] = json!([{ "source": 0 }]);
    }
    let mut json = serde_json::to_vec(&document).unwrap();
    json.resize(json.len().next_multiple_of(4), b' ');
    let mut glb = b"glTF".to_vec();
    glb.extend(2u32.to_le_bytes());
    glb.extend(((12 + 8 + json.len() + 8 + bin.len()) as u32).to_le_bytes());
    glb.extend((json.len() as u32).to_le_bytes());
    glb.extend(b"JSON");
    glb.extend(json);
    glb.extend((bin.len() as u32).to_le_bytes());
    glb.extend(b"BIN\0");
    glb.extend(bin);
    glb
}

/// An unlit material of `color`, in linear light with straight alpha.
fn unlit(name: &str, color: [f32; 4]) -> Value {
    json!({
        "name": name,
        "pbrMetallicRoughness": { "baseColorFactor": color },
        "extensions": { "KHR_materials_unlit": {} },
    })
}

pub(super) fn model(id: u32, params: Value) -> Value {
    json!({ "id": id, "type": "model_3d", "params": params })
}

/// A 1 x 1 PNG of `rgba`, stored without compression.
fn texel_png(rgba: [u8; 4]) -> Vec<u8> {
    let crc = |bytes: &[u8]| {
        let mut crc = !0u32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb8_8320 & (crc & 1).wrapping_neg());
            }
        }
        !crc
    };
    let chunk = |kind: &[u8; 4], data: &[u8]| {
        let mut chunk = (data.len() as u32).to_be_bytes().to_vec();
        chunk.extend(kind);
        chunk.extend(data);
        chunk.extend(crc(&chunk[4..]).to_be_bytes());
        chunk
    };
    // One row: no filter, then the texel.
    let row = [0, rgba[0], rgba[1], rgba[2], rgba[3]];
    let (a, b) = row.iter().fold((1u32, 0u32), |(a, b), &byte| {
        (
            (a + u32::from(byte)) % 65521,
            (b + a + u32::from(byte)) % 65521,
        )
    });
    let mut zlib = vec![0x78, 0x01, 0x01, 5, 0, !5, !0];
    zlib.extend(row);
    zlib.extend(((b << 16) | a).to_be_bytes());
    let mut png = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    png.extend(chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]));
    png.extend(chunk(b"IDAT", &zlib));
    png.extend(chunk(b"IEND", &[]));
    png
}

#[test]
fn a_normal_map_tilts_a_surface_as_vertex_normals_tilting_the_same_way_do() {
    let lean = std::f32::consts::FRAC_1_SQRT_2;
    let (right, up) = ([218, 128, 218, 255], [128, 218, 218, 255]);
    // Each case's texel, whether its texture coordinates run right to left, the model's scale
    // along X, and the vertex normal that tilts the surface the same way. Each is seen from in
    // front and, turned about Y, from behind.
    let fronts = [
        ("+X towards +U", right, false, 1.0, [lean, 0.0, lean]),
        ("+Y up the image", up, false, 1.0, [0.0, lean, lean]),
        ("+X with U mirrored", right, true, 1.0, [-lean, 0.0, lean]),
        (
            "+X on a model mirrored along X",
            right,
            false,
            -1.0,
            [lean, 0.0, lean],
        ),
    ];
    let cases = fronts
        .into_iter()
        .flat_map(|case| [(case, false), (case, true)]);
    let cases = cases.map(|((name, texel, mirrored_u, scale_x, normal), behind)| {
        let name = if behind {
            format!("{name}, from behind")
        } else {
            name.to_string()
        };
        let opposite = [-normal[0], -normal[1], normal[2]];
        let quad = |left: f32, material: usize, normal: [f32; 3], mirrored_u: bool| Quad {
            edges: [left, -0.6, left + 1.2, 0.6],
            material,
            normal,
            mirrored_u,
        };
        let glb = model_glb(
            json!([
                { "normalTexture": { "index": 0 }, "pbrMetallicRoughness": { "roughnessFactor": 1 },
                  "doubleSided": true },
                { "pbrMetallicRoughness": { "roughnessFactor": 1 }, "doubleSided": true },
            ]),
            Some(&texel_png(texel)),
            &[
                quad(-1.9, 0, [0.0, 0.0, 1.0], mirrored_u),
                quad(-0.6, 1, normal, false),
                quad(0.7, 1, opposite, false),
            ],
        );
        let scene = render_3d_with(
            json!({ "environment": 0 }),
            vec![
                model(
                    1,
                    json!({
                        "source": "card.glb",
                        "scale": [scale_x, 1, 1],
                        "rotation": [0, if behind { 180 } else { 0 }, 0],
                    }),
                ),
                ortho(2, json!({})),
                json!({ "id": 3, "type": "light_3d", "params": { "rotation": [25, 35, 0] } }),
                merge(4, &[1, 2, 3], json!({})),
            ],
        );
        let across = if behind { -scale_x } else { scale_x };
        let pixel =
            move |frame: &Frame, x: f32| frame.pixel((32.0 + x * across * 16.0) as u32, CENTER);
        Case::new(
            &name,
            scene,
            Expect::Custom(Box::new(move |frames| {
                let (mapped, reference, other) = (
                    pixel(&frames[0], -1.3),
                    pixel(&frames[0], 0.0),
                    pixel(&frames[0], 1.3),
                );
                let differ =
                    |a: [u8; 4], b: [u8; 4]| (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap();
                assert!(
                    differ(reference, other) > 20,
                    "the light tells the leans apart: {reference:?}, {other:?}"
                );
                assert!(
                    differ(mapped, reference) <= 3,
                    "mapped {mapped:?}, vertex normals {reference:?}, opposite {other:?}"
                );
            })),
        )
        .media(TestMedia::default().model("card.glb", glb))
    });
    check(cases.collect());
}

#[test]
fn a_textured_model_shows_its_base_colour_map() {
    check(vec![
        Case::new(
            "the fox is orange",
            render_3d(vec![fox(1, json!({}))]),
            Expect::Custom(Box::new(|frames| {
                let frame = &frames[0];
                let covered: Vec<[u8; 4]> = (0..SIZE.1)
                    .flat_map(|y| (0..SIZE.0).map(move |x| (x, y)))
                    .map(|(x, y)| frame.pixel(x, y))
                    .filter(|pixel| pixel[3] == 255)
                    .collect();
                assert!(
                    covered.len() > 400,
                    "the fox covers {} pixels",
                    covered.len()
                );
                let orange = covered
                    .iter()
                    .filter(|[r, g, b, _]| r > g && *g > b.saturating_add(20))
                    .count();
                assert!(
                    orange * 2 > covered.len(),
                    "{orange} of {} covered pixels are orange",
                    covered.len()
                );
            })),
        )
        .media(fox_media()),
    ]);
}

#[cfg(all(feature = "3d-draco", feature = "3d-meshopt", feature = "3d-ktx2"))]
#[test]
fn compressed_foxes_render_as_the_fox_does() {
    let gpu = gpu();
    let composition: Composition = render_3d(vec![fox(1, json!({ "animation": "Walk" }))])
        .frames(4)
        .fps(4.0)
        .into();
    let frame = |bytes: &[u8]| {
        let media = TestMedia::default().model("fox.glb", bytes.to_vec());
        render(&mut gpu.renderer(), &composition, 1.0, &media).rgba
    };
    let plain = frame(include_bytes!("../fixtures/Fox.glb"));
    for (name, bytes) in [
        (
            "Fox-draco",
            include_bytes!("../fixtures/Fox-draco.glb").as_slice(),
        ),
        (
            "Fox-meshopt-ktx2",
            include_bytes!("../fixtures/Fox-meshopt-ktx2.glb").as_slice(),
        ),
    ] {
        let compressed = frame(bytes);
        let sum: u64 = plain
            .iter()
            .zip(&compressed)
            .map(|(a, b)| u64::from(a.abs_diff(*b)))
            .sum();
        // Running rather than walking differs by about 14.
        let difference = sum as f64 / plain.len() as f64;
        assert!(difference < 2.0, "{name}: {difference}");
    }
}

#[test]
fn a_skinned_model_playing_a_clip_poses_anew_each_frame_and_a_still_one_is_kept() {
    let gpu = gpu();
    let frames = |params: Value| {
        let composition: Composition = render_3d(vec![fox(1, params)]).frames(4).fps(4.0).into();
        let media = fox_media();
        let mut renderer = gpu.renderer();
        let frames: Vec<Vec<u8>> = (0..4)
            .map(|frame| render(&mut renderer, &composition, f64::from(frame), &media).rgba)
            .collect();
        assert_eq!(media.model_loads(), 1, "the model loads once");
        (frames, renderer.skipped_pass_count())
    };

    let (still, skipped) = frames(json!({}));
    assert!(still.iter().all(|frame| *frame == still[0]));
    assert!(
        skipped >= 3 * 2,
        "frames after the first read the kept draw and resolve, skipping {skipped} passes"
    );
    let (paused, skipped) = frames(json!({ "animation": "Walk", "animation_speed": 0 }));
    assert!(paused.iter().all(|frame| *frame == paused[0]));
    assert!(
        skipped >= 3 * 2,
        "a clip at speed 0 is kept too, skipping {skipped} passes"
    );

    let (walking, skipped) = frames(json!({ "animation": "Walk" }));
    assert_eq!(skipped, 0, "a model playing a clip renders every frame");
    for pair in walking.windows(2) {
        assert!(pair[0] != pair[1], "the fox walks");
    }
    assert!(
        walking[0] != still[0],
        "walking moves the fox from its rest pose"
    );
}

#[test]
fn two_models_sharing_a_source_load_it_once_and_play_their_own_clips() {
    let gpu = gpu();
    // A walking fox on the left, and one playing `right` on the right.
    let pair = |right: &str| {
        let fox_at = |id: u32, x: f64, animation: &str| {
            fox(
                id,
                json!({ "animation": animation, "position": [x, -0.4, 0], "scale": [0.008, 0.008, 0.008] }),
            )
        };
        let composition: Composition = render_3d(vec![
            fox_at(1, -0.9, "Walk"),
            fox_at(2, 0.9, right),
            merge(3, &[1, 2], json!({})),
        ])
        .frames(4)
        .fps(4.0)
        .into();
        let media = fox_media();
        let frame = render(&mut gpu.renderer(), &composition, 1.0, &media).rgba;
        assert_eq!(
            media.model_loads(),
            1,
            "both foxes draw one load of the model"
        );
        frame
    };
    let half = |rgba: &[u8], right: bool| -> Vec<u8> {
        rgba.chunks(SIZE.0 as usize * 4)
            .flat_map(|row| {
                let (left, rest) = row.split_at(row.len() / 2);
                if right { rest } else { left }.to_vec()
            })
            .collect()
    };
    let (walking, running) = (pair("Walk"), pair("Run"));
    assert!(
        half(&walking, false).iter().any(|&value| value != 0),
        "the left fox shows"
    );
    assert_eq!(
        half(&walking, false),
        half(&running, false),
        "the left fox walks in both"
    );
    assert_ne!(
        half(&walking, true),
        half(&running, true),
        "the right fox runs in one"
    );
}

#[test]
fn the_texture_input_shows_on_the_named_material_as_an_image_plane_shows_it() {
    let screen = |quads_of: &[([f32; 4], usize)]| {
        quads(
            json!([
                unlit("body", [1.0, 0.0, 0.0, 1.0]),
                unlit("screen", [0.0, 1.0, 0.0, 1.0])
            ]),
            quads_of,
        )
    };
    let seen = |model_bytes: Vec<u8>, media: TestMedia| {
        let mut nodes = vec![plate(10)];
        nodes.push(json!({
            "id": 1,
            "type": "model_3d",
            "params": { "source": "tv.glb", "texture_material": "screen" },
            "in": { "texture": 10 },
        }));
        nodes.extend([ortho(2, json!({})), merge(3, &[1, 2], json!({}))]);
        (
            render_3d_with(json!({ "supersample": 1 }), nodes),
            media.model("tv.glb", model_bytes),
        )
    };

    let (full, full_media) = seen(screen(&[([-2.0, -2.0, 2.0, 2.0], 1)]), gradient());
    // The raster's left half is green and its right half blue; the screen shows it on the left
    // half of the frame, beside the red body.
    let halves = image(|x, _| if x < 32 { GREEN } else { BLUE });
    let (beside, beside_media) = seen(
        screen(&[([-2.0, -2.0, 0.0, 2.0], 1), ([0.0, -2.0, 2.0, 2.0], 0)]),
        halves,
    );
    check(vec![
        Case::new(
            "filling the frame, one texel to a pixel",
            full,
            Expect::Matches(scene(SIZE, json!([plate(1)])).into()),
        )
        .media(full_media)
        .tolerance(1),
        Case::new(
            "beside another material",
            beside,
            Expect::Probes(vec![
                (8, CENTER, GREEN),
                (24, CENTER, BLUE),
                (48, CENTER, RED),
            ]),
        )
        .media(beside_media),
    ]);
}

#[test]
fn masked_materials_cut_out_and_blended_ones_show_through() {
    let covering = |material: Value| {
        let media = TestMedia::default().model(
            "card.glb",
            quads(json!([material]), &[([-3.0, -3.0, 3.0, 3.0], 0)]),
        );
        let scene = render_3d(vec![
            model(1, json!({ "source": "card.glb" })),
            ortho(2, json!({})),
            merge(3, &[1, 2], json!({})),
        ]);
        (scene, media)
    };
    let with = |alpha: f32, mode: Value| {
        let mut material = unlit("card", [0.0, 0.0, 1.0, alpha]);
        for (name, value) in mode.as_object().expect("mode") {
            material[name] = value.clone();
        }
        covering(material)
    };
    let case = |name: &str, (scene, media): (Scene, TestMedia), expect| {
        Case::new(name, scene, expect).media(media)
    };
    check(vec![
        case(
            "masked below its cutoff",
            with(0.4, json!({ "alphaMode": "MASK", "alphaCutoff": 0.5 })),
            Expect::Uniform(CLEAR),
        ),
        case(
            "masked at its cutoff, and opaque there",
            with(0.5, json!({ "alphaMode": "MASK", "alphaCutoff": 0.5 })),
            Expect::Uniform(BLUE),
        ),
        case(
            "blended at half",
            with(0.5, json!({ "alphaMode": "BLEND" })),
            Expect::Uniform([0, 0, 128, 128]),
        ),
        case(
            "opaque ignores alpha",
            with(0.25, json!({})),
            Expect::Uniform(BLUE),
        ),
    ]);
}

#[test]
fn a_clip_or_material_the_model_lacks_is_an_error_listing_those_it_has() {
    let error = |params: Value| {
        let composition: Composition = render_3d(vec![fox(1, params)]).into();
        gpu()
            .renderer()
            .render_frame_at(&composition, 0.0, &fox_media())
            .expect_err("the render fails")
            .to_string()
    };
    assert_eq!(
        error(json!({ "animation": "Dance" })),
        "Model3d node 1 evaluation failed: model `fox.glb`: the model has no animation `Dance`; \
         it has 0 `Survey`, 1 `Walk`, 2 `Run`"
    );
    assert_eq!(
        error(json!({ "texture_material": "screen" })),
        "Model3d node 1 evaluation failed: model `fox.glb`: the model has no material `screen`; \
         it has `fox_material`"
    );
}

/// Where the ground below is seen: an orthographic camera looking straight down at `center`,
/// `height` units across, which leaves out everything above 0.05 so that only the ground and
/// the shadows on it show.
struct GroundView {
    center: [f32; 2],
    height: f32,
}

impl GroundView {
    const WHOLE: Self = Self {
        center: [0.0, 0.0],
        height: 6.0,
    };

    /// `objects` over a white, fully rough ground 6 units across at y = 0, without light from
    /// all around.
    fn scene(&self, objects: Vec<Value>, render: Value) -> Scene {
        scene(SIZE, json!(self.nodes(objects, render)))
    }

    /// The nodes of [`Self::scene`], the `render_3d` last, with id 100.
    fn nodes(&self, objects: Vec<Value>, render: Value) -> Vec<Value> {
        let [x, z] = self.center;
        let mut nodes = vec![
            json!({ "id": 50, "type": "shape_3d", "params": {
                "shape": "plane", "size": [6, 1, 6], "roughness": 1 } }),
            json!({ "id": 51, "type": "camera_3d", "params": {
                "projection": "orthographic", "ortho_height": self.height, "position": [x, 4, z],
                "rotation": [-90, 0, 0], "near": 3.95, "far": 10 } }),
        ];
        nodes.extend(objects);
        let ids: Vec<u32> = nodes
            .iter()
            .filter(|node| {
                node["type"]
                    .as_str()
                    .is_some_and(|kind| kind.ends_with("_3d"))
            })
            .map(|node| node["id"].as_u64().unwrap() as u32)
            .collect();
        nodes.push(merge(90, &ids, json!({})));
        let mut params = json!({ "environment": 0 });
        for (name, value) in render.as_object().expect("params") {
            params[name] = value.clone();
        }
        nodes.push(
            json!({ "id": 100, "type": "render_3d", "params": params, "in": { "scene": 90 } }),
        );
        nodes
    }

    /// The pixel showing the ground at `x`, `z`.
    fn pixel(&self, x: f32, z: f32) -> (u32, u32) {
        let at = |value: f32, center: f32| ((value - center) / self.height + 0.5) * SIZE.0 as f32;
        (at(x, self.center[0]) as u32, at(z, self.center[1]) as u32)
    }

    /// Whether pixel `(x, y)` lies `margin` pixels or more inside the ground's `left`, `top`,
    /// `right` and `bottom`, or outside them for a negative margin.
    fn inside(
        &self,
        (x, y): (u32, u32),
        [left, top, right, bottom]: [f32; 4],
        margin: f32,
    ) -> bool {
        let (x0, y0) = self.pixel(left, top);
        let (x1, y1) = self.pixel(right, bottom);
        let (x, y) = (x as f32 + 0.5, y as f32 + 0.5);
        x >= x0 as f32 + margin
            && x <= x1 as f32 + 1.0 - margin
            && y >= y0 as f32 + margin
            && y <= y1 as f32 + 1.0 - margin
    }
}

pub(super) fn light(id: u32, params: Value) -> Value {
    json!({ "id": id, "type": "light_3d", "params": params })
}

/// `params` with shadows cast or not.
fn casting(params: &Value, cast: bool) -> Value {
    let mut params = params.clone();
    params["cast_shadows"] = json!(cast);
    params
}

pub(super) fn frame_of(scene: Scene, media: &TestMedia) -> Frame {
    render(&mut gpu().renderer(), &scene.into(), 0.0, media)
}

fn every_pixel() -> impl Iterator<Item = (u32, u32)> {
    (0..SIZE.1).flat_map(|y| (0..SIZE.0).map(move |x| (x, y)))
}

#[test]
fn each_kind_of_light_darkens_where_its_shadow_falls_and_nothing_else() {
    let view = GroundView::WHOLE;
    // A slab 2 units up, which each light shines past at 45° towards +X onto (1, 0, 0). The
    // shadow's extent on the ground, left, top, right and bottom: the directional light moves
    // the slab over, and the others, two units farther from the ground than the slab is, also
    // spread it about twice as wide.
    let slab = json!({ "id": 1, "type": "shape_3d", "params": {
        "size": [0.6, 0.2, 0.6], "position": [-1, 2, 0] } });
    let spread = [0.24, -0.63, 1.84, 0.63];
    let lights = [
        (
            "directional",
            json!({ "light": "directional", "intensity": 3, "rotation": [-45, -90, 0] }),
            [0.6, -0.3, 1.4, 0.3],
        ),
        (
            "spot",
            json!({ "light": "spot", "intensity": 40, "position": [-3, 4, 0],
                    "rotation": [-45, -90, 0], "spot_angle": 60 }),
            spread,
        ),
        (
            "point",
            json!({ "light": "point", "intensity": 40, "position": [-3, 4, 0] }),
            spread,
        ),
    ];
    let media = TestMedia::default();
    for (name, params, shadow) in lights {
        let seen = |with_slab: bool, cast: bool| {
            let mut objects = vec![light(60, casting(&params, cast))];
            if with_slab {
                objects.push(slab.clone());
            }
            frame_of(view.scene(objects, json!({})), &media)
        };
        let (shadowed, unshadowed, bare) = (seen(true, true), seen(true, false), seen(false, true));
        assert!(
            unshadowed.rgba == bare.rgba,
            "{name}: a light without cast_shadows lights the ground as if nothing were above it"
        );
        let (x, y) = view.pixel(1.0, 0.0);
        let [lit, ..] = bare.pixel(x, y);
        assert!(lit > 60, "{name}: the ground is lit, at {lit}");
        assert_eq!(
            shadowed.pixel(x, y),
            [0, 0, 0, 255],
            "{name}: the shadow's middle"
        );
        for at in every_pixel() {
            let (before, after) = (bare.pixel(at.0, at.1), shadowed.pixel(at.0, at.1));
            if view.inside(at, shadow, -1.5) {
                if view.inside(at, shadow, 1.5) {
                    assert!(
                        after[0] < 8,
                        "{name}: {at:?} is in the shadow, at {}",
                        after[0]
                    );
                }
            } else {
                assert!(
                    before.iter().zip(after).all(|(a, b)| a.abs_diff(b) <= 1),
                    "{name}: {at:?} is outside the shadow, but changed from {before:?} to {after:?}"
                );
            }
        }
    }
}

#[test]
fn ground_lit_at_a_grazing_angle_shadows_nothing_of_itself() {
    let view = GroundView::WHOLE;
    let lights = [
        (
            "a directional light 5° above the horizon",
            json!({ "light": "directional", "intensity": 10, "rotation": [-5, -90, 0] }),
        ),
        (
            "a point light just above the ground",
            json!({ "light": "point", "intensity": 3, "position": [-2.5, 0.25, 0.5] }),
        ),
    ];
    check(
        lights
            .into_iter()
            .map(|(name, params)| {
                let seen = |cast| view.scene(vec![light(60, casting(&params, cast))], json!({}));
                Case::new(name, seen(true), Expect::Matches(seen(false).into()))
            })
            .collect(),
    );
}

#[test]
fn a_box_on_the_ground_is_shadowed_from_its_base_without_a_gap() {
    // The box's right side stands at x = 0.3, and its shadow reaches 0.6 farther in light 45°
    // from overhead. Each pixel is 2.5 cm.
    let view = GroundView {
        center: [0.3, 0.0],
        height: 1.6,
    };
    let scene = view.scene(
        vec![
            json!({ "id": 1, "type": "shape_3d", "params": {
                "size": [0.6, 0.6, 0.6], "position": [0, 0.3, 0] } }),
            light(
                60,
                json!({ "light": "directional", "intensity": 3, "rotation": [-45, -90, 0],
                        "cast_shadows": true }),
            ),
        ],
        json!({}),
    );
    let frame = frame_of(scene, &TestMedia::default());
    let (edge, y) = view.pixel(0.3, 0.0);
    let (end, _) = view.pixel(0.9, 0.0);
    let row: Vec<u8> = (0..SIZE.0).map(|x| frame.pixel(x, y)[0]).collect();
    assert!(
        row[edge as usize..end as usize - 1].iter().all(|&r| r < 8),
        "the shadow runs dark from the box's base, columns {edge} to {end}: {row:?}"
    );
    assert!(
        row[end as usize + 2..].iter().all(|&r| r > 60),
        "and the ground past it is lit: {row:?}"
    );
}

#[test]
fn a_walking_models_shadow_takes_its_posed_shape() {
    let view = GroundView {
        center: [0.0, 0.0],
        height: 2.4,
    };
    // Seen from above by a camera that leaves out the fox, the shadow of light from straight
    // overhead is the outline of the fox seen from above by one that leaves out the ground.
    let seen = |at: f64, shadow: bool| {
        let fox = fox(
            1,
            json!({ "position": [0, 0, 0], "rotation": [0, 30, 0], "scale": [0.018, 0.018, 0.018],
                    "animation": "Walk" }),
        );
        let sun = light(
            60,
            json!({ "light": "directional", "rotation": [-90, 0, 0], "cast_shadows": true,
                    "shadow_softness": 0 }),
        );
        let mut scene = view.scene(vec![fox, sun], json!({})).frames(30);
        if !shadow {
            let camera = scene.0["nodes"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|node| node["id"] == 51)
                .unwrap();
            camera["params"]["near"] = json!(0.05);
            camera["params"]["far"] = json!(3.9);
        }
        let frame = render(&mut gpu().renderer(), &scene.into(), at, &fox_media());
        // A pixel at least half in shadow has under half the light of the lit ground in its
        // corner, which in sRGB is under about 0.73 of its value.
        let lit = f32::from(frame.pixel(0, SIZE.1 - 1)[0]);
        frame
            .rgba
            .chunks(4)
            .map(|pixel| {
                if shadow {
                    f32::from(pixel[0]) < lit * 0.73
                } else {
                    pixel[3] >= 128
                }
            })
            .collect::<Vec<bool>>()
    };
    let differing = |a: &[bool], b: &[bool]| a.iter().zip(b).filter(|(a, b)| a != b).count();
    let (outline, later) = (seen(0.0, false), seen(12.0, false));
    let area = outline.iter().filter(|&&covered| covered).count();
    assert!(area > 200, "the fox covers {area} pixels");
    assert!(
        differing(&outline, &later) > area / 20,
        "the fox moves between the frames"
    );
    for (at, own, other) in [(0.0, &outline, &later), (12.0, &later, &outline)] {
        let shadow = seen(at, true);
        let off = differing(&shadow, own);
        assert!(
            off <= area / 20 && off < differing(&shadow, other),
            "frame {at}: the shadow and outline differ at {off} of {area} pixels"
        );
    }
}

#[test]
fn masked_and_blended_surfaces_cast_shadows_only_where_they_cover() {
    let view = GroundView::WHOLE;
    let sun = || {
        light(
            60,
            json!({ "light": "directional", "intensity": 3, "rotation": [-90, 0, 0],
                    "cast_shadows": true }),
        )
    };
    // A card filling the middle of the ground, a unit up and facing it.
    let card = |material: Value| {
        let mut material = material;
        material["name"] = json!("card");
        let media = TestMedia::default().model(
            "card.glb",
            quads(json!([material]), &[([-1.0, -1.0, 1.0, 1.0], 0)]),
        );
        let model = model(
            1,
            json!({ "source": "card.glb", "position": [0, 1, 0], "rotation": [-90, 0, 0] }),
        );
        frame_of(view.scene(vec![model, sun()], json!({})), &media)
    };
    let (middle_x, middle_y) = view.pixel(0.0, 0.0);
    let middle = |frame: Frame| frame.pixel(middle_x, middle_y)[0];
    let alpha = |alpha: f32, mode: Value| {
        let mut material =
            json!({ "pbrMetallicRoughness": { "baseColorFactor": [1, 1, 1, alpha] } });
        for (name, value) in mode.as_object().expect("mode") {
            material[name] = value.clone();
        }
        middle(card(material))
    };
    let mask = json!({ "alphaMode": "MASK", "alphaCutoff": 0.5 });
    let blend = json!({ "alphaMode": "BLEND" });
    assert!(
        alpha(0.4, mask.clone()) > 60,
        "masked below its cutoff, it casts none"
    );
    assert_eq!(
        alpha(0.5, mask),
        0,
        "masked at its cutoff, it casts all of it"
    );
    assert!(
        alpha(0.4, blend.clone()) > 60,
        "blended below half, it casts none"
    );
    assert_eq!(
        alpha(0.6, blend),
        0,
        "blended over half, it casts all of it"
    );

    // An image plane blends by its raster, whose left half is opaque and right half transparent.
    let plane = plane(
        1,
        10,
        json!({ "width": 2, "position": [0, 1, 0], "rotation": [-90, 0, 0] }),
    );
    let frame = frame_of(
        view.scene(vec![plate(10), plane, sun()], json!({})),
        &half_blue(),
    );
    let at = |x: f32| {
        let (x, y) = view.pixel(x, 0.0);
        frame.pixel(x, y)[0]
    };
    assert_eq!(
        (at(-0.5), at(0.5) > 60),
        (0, true),
        "the opaque half alone casts a shadow"
    );
}

#[test]
fn a_shadow_catcher_darkens_what_is_under_it_by_the_light_its_shadows_take_away() {
    const BACKDROP: [u8; 4] = [200, 100, 50, 255];
    let view = GroundView::WHOLE;
    // A slab a unit over the middle of a catcher, lit from straight above, and with as much
    // light again from all around, so that its shadow takes half the light away.
    let nodes = |render: Value, catcher: Value| {
        let mut catcher_params = json!({
            "shape": "plane", "size": [6, 1, 6], "shadow_catcher": true, "position": [0, 0.01, 0] });
        for (name, value) in catcher.as_object().expect("catcher") {
            catcher_params[name] = value.clone();
        }
        let objects = vec![
            json!({ "id": 1, "type": "shape_3d", "params": { "size": [1, 0.2, 1], "position": [0, 1, 0] } }),
            json!({ "id": 2, "type": "shape_3d", "params": catcher_params }),
            light(
                60,
                json!({ "light": "directional", "intensity": 1, "rotation": [-90, 0, 0],
                             "cast_shadows": true }),
            ),
            light(
                61,
                json!({ "light": "ambient", "intensity": 1.0 / std::f64::consts::PI }),
            ),
        ];
        view.nodes(objects, render)
    };
    let scene =
        |render: Value, catcher: Value| crate::support::scene(SIZE, json!(nodes(render, catcher)));
    let (middle_x, middle_y) = view.pixel(0.0, 0.0);
    let (edge_x, edge_y) = view.pixel(2.5, 2.5);
    let probes = |middle: [u8; 4], edge: [u8; 4]| {
        Expect::Probes(vec![(middle_x, middle_y, middle), (edge_x, edge_y, edge)])
    };
    let mut over_backdrop =
        vec![json!({ "id": 200, "type": "background", "params": { "paint": BACKDROP } })];
    over_backdrop.extend(nodes(json!({}), json!({})));
    over_backdrop
        .push(json!({ "id": 201, "type": "merge", "in": { "base": 200, "overlay": 100 } }));
    let half = [0, 0, 0, 128];
    // Half the backdrop shows through half-transparent black.
    let darkened = [100, 50, 25, 255];
    check(vec![
        Case::new(
            "transparent where unshadowed, half-transparent black in the shadow",
            scene(json!({}), json!({})),
            probes(half, CLEAR),
        )
        .tolerance(1),
        Case::new(
            "whatever the exposure and tone mapping",
            scene(json!({ "exposure": 2, "tone_mapping": "aces" }), json!({})),
            probes(half, CLEAR),
        )
        .tolerance(1),
        Case::new(
            "and the supersampling",
            scene(json!({ "supersample": 1 }), json!({})),
            probes(half, CLEAR),
        )
        .tolerance(1),
        Case::new(
            "its opacity scales how dark it is",
            scene(json!({}), json!({ "opacity": 0.5 })),
            probes([0, 0, 0, 64], CLEAR),
        )
        .tolerance(1),
        Case::new(
            "composited over a backdrop",
            crate::support::scene(SIZE, json!(over_backdrop)),
            probes(darkened, BACKDROP),
        )
        .tolerance(1),
    ]);
}

#[test]
fn shadow_catchers_hide_what_is_behind_them() {
    let view = GroundView::WHOLE;
    // A red box sunk below a catcher shows nowhere.
    let scene = view.scene(
        vec![
            flat(1, json!({ "position": [0, -0.6, 0] }), RED),
            json!({ "id": 2, "type": "shape_3d", "params": {
                "shape": "plane", "size": [6, 1, 6], "shadow_catcher": true, "position": [0, 0.01, 0] } }),
        ],
        json!({}),
    );
    check(vec![Case::new(
        "a box under a catcher",
        scene,
        Expect::Uniform(CLEAR),
    )]);
}

#[test]
fn lights_past_the_first_eight_casting_shadows_cast_none_and_are_named_in_a_warning() {
    let view = GroundView::WHOLE;
    let slab = json!({ "id": 1, "type": "shape_3d", "params": {
        "size": [1, 0.2, 1], "position": [0, 1, 0] } });
    // Eight dark lights casting shadows, then the one that lights the ground.
    let mut objects: Vec<Value> = (60..68)
        .map(|id| {
            light(
                id,
                json!({ "light": "directional", "intensity": 0, "cast_shadows": true }),
            )
        })
        .collect();
    objects.push(light(
        68,
        json!({ "light": "directional", "intensity": 3, "rotation": [-90, 0, 0],
                "cast_shadows": true }),
    ));
    let composition: Composition = view.scene(objects.clone(), json!({})).into();
    let mut renderer = gpu().renderer();
    let frame = render(&mut renderer, &composition, 0.0, &TestMedia::default());
    let warnings = renderer.take_diagnostics();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].code, "scene.shadow_limit");
    assert_eq!(warnings[0].node, Some(lumen_engine::node::NodeId::new(100)));
    assert!(
        warnings[0]
            .message
            .contains("light nodes 68 cast no shadows"),
        "{}",
        warnings[0].message
    );
    let (x, y) = view.pixel(0.0, 0.0);
    assert!(
        frame.pixel(x, y)[0] > 60,
        "the last light lights the ground under the slab"
    );

    objects.push(slab);
    let with_slab = frame_of(view.scene(objects, json!({})), &TestMedia::default());
    assert_eq!(with_slab.pixel(x, y), frame.pixel(x, y));
}

#[test]
fn shadows_of_lights_and_objects_too_far_or_large_to_place_draw_nothing() {
    let view = GroundView::WHOLE;
    let objects = vec![
        json!({ "id": 1, "type": "shape_3d", "params": { "size": [1, 0.2, 1], "position": [0, 1, 0] } }),
        json!({ "id": 2, "type": "shape_3d", "params": { "size": [1e30, 1e30, 1e30], "position": [0, 1, 0] } }),
        json!({ "id": 3, "type": "shape_3d", "params": { "position": [1e300, 0, 0] } }),
        light(
            60,
            json!({ "light": "point", "intensity": 3, "position": [1e300, 2, 0], "cast_shadows": true,
                         "shadow_softness": 1e300 }),
        ),
        light(
            61,
            json!({ "light": "directional", "intensity": 3, "rotation": [1e300, 0, 0], "cast_shadows": true }),
        ),
        light(
            62,
            json!({ "light": "spot", "intensity": 3, "position": [0, 3, 0], "rotation": [-90, 0, 0],
                         "spot_angle": 0, "cast_shadows": true }),
        ),
    ];
    frame_of(view.scene(objects, json!({})), &TestMedia::default());
}

#[test]
fn an_image_plane_catches_shadows_where_its_raster_covers_it_and_shows_none_of_its_colour() {
    let view = GroundView::WHOLE;
    let scene = |catcher: bool| {
        let mut objects = vec![
            json!({ "id": 1, "type": "shape_3d", "params": { "size": [1, 0.2, 1], "position": [0, 1, 0] } }),
            light(
                60,
                json!({ "light": "directional", "intensity": 1, "rotation": [-90, 0, 0],
                        "cast_shadows": true }),
            ),
            light(
                61,
                json!({ "light": "ambient", "intensity": 1.0 / std::f64::consts::PI }),
            ),
            plate(10),
        ];
        if catcher {
            objects.push(plane(
                2,
                10,
                json!({ "width": 6, "rotation": [-90, 0, 0], "position": [0, 0.01, 0],
                        "shadow_catcher": true }),
            ));
        }
        view.scene(objects, json!({}))
    };
    let (middle_x, middle_y) = view.pixel(0.0, 0.0);
    let (edge_x, edge_y) = view.pixel(2.5, 2.5);

    // Unlit by default, an opaque red source catches shadows as a shape does.
    let red = frame_of(scene(true), &image(|_, _| RED));
    for (pixel, expected) in [
        (red.pixel(middle_x, middle_y), [0, 0, 0, 128]),
        (red.pixel(edge_x, edge_y), CLEAR),
    ] {
        assert!(
            pixel.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 1),
            "{pixel:?}, not {expected:?}"
        );
    }
    // A transparent source leaves the ground under the plane showing as if it weren't there.
    let transparent = image(|_, _| CLEAR);
    let (with, without) = (
        frame_of(scene(true), &transparent),
        frame_of(scene(false), &transparent),
    );
    assert!(
        with.rgba
            .iter()
            .zip(&without.rgba)
            .all(|(a, b)| a.abs_diff(*b) <= 1)
    );
    assert_eq!(with.pixel(edge_x, edge_y)[3], 255);
}

/// The pixels a light's shadows leave below half as bright as `lit`, the same scene unshadowed.
fn darkened(shadowed: &Frame, lit: &Frame) -> Vec<(u32, u32)> {
    let (width, height) = (lit.width, lit.height);
    (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let (a, b) = (shadowed.pixel(x, y)[0], lit.pixel(x, y)[0]);
            b > 30 && a < b / 2
        })
        .collect()
}

/// The mean place of `pixels`, and their bounds: left, top, right and bottom.
fn extent(pixels: &[(u32, u32)]) -> ([f32; 2], [u32; 4]) {
    let n = pixels.len().max(1) as f32;
    let mean = [
        pixels.iter().map(|p| p.0 as f32).sum::<f32>() / n,
        pixels.iter().map(|p| p.1 as f32).sum::<f32>() / n,
    ];
    let bounds = [
        pixels.iter().map(|p| p.0).min().unwrap_or(0),
        pixels.iter().map(|p| p.1).min().unwrap_or(0),
        pixels.iter().map(|p| p.0).max().unwrap_or(0),
        pixels.iter().map(|p| p.1).max().unwrap_or(0),
    ];
    (mean, bounds)
}

#[test]
fn shadows_of_small_objects_on_a_floor_far_wider_than_the_view_are_kept() {
    let lights = [
        json!({ "light": "directional", "intensity": 3, "rotation": [-50, 35, 0] }),
        json!({ "light": "spot", "intensity": 60, "position": [0.5, 4, 0.5], "rotation": [-80, 45, 0],
                "spot_angle": 60 }),
        json!({ "light": "point", "intensity": 30, "position": [0.5, 3, 0.5] }),
    ];
    for params in lights {
        // The box's shadow on a 6 m patch over a floor `half` units out each way, which casts
        // none and places its own pixels only to within about a centimetre when kilometres across.
        let shadow = |half: f64| {
            let frame = |cast: bool| {
                let nodes = vec![
                    json!({ "id": 1, "type": "shape_3d", "params": {
                        "shape": "plane", "size": [2.0 * half, 1, 2.0 * half], "roughness": 1,
                        "cast_shadows": false } }),
                    json!({ "id": 6, "type": "shape_3d", "params": {
                        "shape": "plane", "size": [6, 1, 6], "position": [0, 0.02, 0],
                        "roughness": 1, "cast_shadows": false } }),
                    json!({ "id": 2, "type": "shape_3d", "params": {
                        "size": [0.6, 0.6, 0.6], "position": [0, 0.32, 0] } }),
                    json!({ "id": 3, "type": "camera_3d", "params": {
                        "position": [0, 1.5, 3], "look_at": true, "target": [0, 0, 0] } }),
                    light(4, casting(&params, cast)),
                    merge(5, &[1, 6, 2, 3, 4], json!({})),
                ];
                frame_of(
                    render_3d_with(json!({ "environment": 0 }), nodes),
                    &TestMedia::default(),
                )
            };
            darkened(&frame(true), &frame(false))
        };
        let small = shadow(3.0);
        assert!(
            small.len() > 12,
            "{params}: {} pixels shadowed",
            small.len()
        );
        let (mean, bounds) = extent(&small);
        for half in [500.0, 5000.0] {
            let (wide_mean, wide_bounds) = extent(&shadow(half));
            assert!(
                (wide_mean[0] - mean[0]).abs() <= 1.0 && (wide_mean[1] - mean[1]).abs() <= 1.0,
                "{params}: the shadow's middle on a floor {half} out is {wide_mean:?}, not {mean:?}"
            );
            assert!(
                wide_bounds
                    .iter()
                    .zip(bounds)
                    .all(|(a, b)| a.abs_diff(b) <= 2),
                "{params}: the shadow on a floor {half} out spans {wide_bounds:?}, not {bounds:?}"
            );
        }
    }
}

#[test]
fn point_and_wide_spot_lights_shadow_casters_off_their_views_axes() {
    let view = GroundView::WHOLE;
    // A small box a metre up, which a light at (2, 3, 2) shadows around (-1, 0, -1): much nearer
    // along the view it falls in than in a straight line, and alone, as the floor casts nothing.
    let lights = [
        json!({ "light": "point", "intensity": 100, "position": [2, 3, 2] }),
        json!({ "light": "spot", "intensity": 100, "position": [2, 3, 2], "rotation": [-90, 0, 0],
                "spot_angle": 160 }),
    ];
    for params in lights {
        let frame = |cast: bool| {
            let mut nodes = view.nodes(
                vec![
                    json!({ "id": 1, "type": "shape_3d", "params": {
                        "size": [0.3, 0.3, 0.3], "position": [0, 1, 0] } }),
                    light(60, casting(&params, cast)),
                ],
                json!({}),
            );
            nodes[0]["params"]["cast_shadows"] = json!(false);
            frame_of(scene(SIZE, json!(nodes)), &TestMedia::default())
        };
        let (shadowed, lit) = (frame(true), frame(false));
        let (x, y) = view.pixel(-1.0, -1.0);
        let ([a, ..], [b, ..]) = (shadowed.pixel(x, y), lit.pixel(x, y));
        assert!(b > 30 && a < b / 4, "{params}: {a} where unshadowed is {b}");
    }
}

#[test]
fn a_directional_lights_shadows_fade_out_toward_its_shadow_distance() {
    let view = GroundView::WHOLE;
    // The ground lies 4 units along the view from the camera looking straight down at it.
    let darkness = |distance: f64| {
        let frame = |cast: bool| {
            let objects = vec![
                json!({ "id": 1, "type": "shape_3d", "params": { "size": [1, 0.2, 1], "position": [0, 1, 0] } }),
                light(
                    60,
                    json!({ "light": "directional", "intensity": 1, "rotation": [-90, 0, 0],
                            "cast_shadows": cast, "shadow_distance": distance }),
                ),
            ];
            frame_of(view.scene(objects, json!({})), &TestMedia::default())
        };
        let (x, y) = view.pixel(0.0, 0.0);
        let ([a, ..], [b, ..]) = (frame(true).pixel(x, y), frame(false).pixel(x, y));
        assert!(b > 60, "the ground is lit, at {b}");
        1.0 - f32::from(a) / f32::from(b)
    };
    assert!(darkness(50.0) > 0.95, "well within: {}", darkness(50.0));
    // A tenth of 4.2 is 0.42, of which the ground lies 0.2 inside, so the shadow takes about half
    // the light, which shows a quarter darker once encoded for display.
    let partial = darkness(4.2);
    assert!((0.15..0.4).contains(&partial), "in the fade: {partial}");
    assert_eq!(darkness(3.9), 0.0, "beyond");
}

#[test]
fn how_far_directional_shadows_reach_doesnt_depend_on_their_softness() {
    // Boxes at increasing distances along a floor to the horizon, which casts nothing.
    let frame = |softness: f64, cast: bool| {
        let mut nodes = vec![
            json!({ "id": 1, "type": "shape_3d", "params": {
                "shape": "plane", "size": [10000, 1, 10000], "roughness": 1, "cast_shadows": false } }),
            json!({ "id": 3, "type": "camera_3d", "params": {
                "position": [0, 10, 12], "look_at": true, "target": [0, 0, -25] } }),
            light(
                4,
                json!({ "light": "directional", "intensity": 3, "rotation": [-50, 35, 0],
                        "cast_shadows": cast, "shadow_softness": softness }),
            ),
        ];
        let mut ids = vec![1, 3, 4];
        for (index, z) in [-10, -18, -26, -34].into_iter().enumerate() {
            let id = 10 + index as u32;
            let x = if index % 2 == 0 { 5 } else { -5 };
            nodes.push(json!({ "id": id, "type": "shape_3d", "params": {
                "size": [2, 1, 2], "position": [x, 0.5, z], "receive_shadows": false } }));
            ids.push(id);
        }
        nodes.push(merge(5, &ids, json!({})));
        nodes.push(json!({ "id": 100, "type": "render_3d", "params": { "environment": 0 }, "in": { "scene": 5 } }));
        frame_of(scene((128, 128), json!(nodes)), &TestMedia::default())
    };
    let lit = frame(0.0, false);
    let shadows = [0.0, 0.3].map(|softness| darkened(&frame(softness, true), &lit));
    let ((sharp_mean, sharp_bounds), (soft_mean, soft_bounds)) =
        (extent(&shadows[0]), extent(&shadows[1]));
    assert!(
        shadows[0].len() > 20,
        "{} pixels shadowed",
        shadows[0].len()
    );
    assert!(
        (sharp_mean[0] - soft_mean[0]).abs() <= 1.5 && (sharp_mean[1] - soft_mean[1]).abs() <= 1.5,
        "{sharp_mean:?} sharp, {soft_mean:?} soft"
    );
    assert!(
        sharp_bounds
            .iter()
            .zip(soft_bounds)
            .all(|(a, b)| a.abs_diff(b) <= 3),
        "{sharp_bounds:?} sharp, {soft_bounds:?} soft"
    );
}

/// A `width` by `height` equirectangular `.hdr` whose texels hold `radiance` of the direction
/// through their centres.
fn environment_file(width: u32, height: u32, radiance: impl Fn(Vec3) -> Vec3) -> Vec<u8> {
    let map = Equirect::from_fn(width, height, radiance);
    let texels: Vec<[f32; 3]> = map.texels().iter().map(|texel| texel.to_array()).collect();
    crate::support::hdr_file(width, height, &texels)
}

/// Linear `value` as an 8-bit sRGB channel.
fn srgb8(value: f32) -> u8 {
    let encoded = if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round() as u8
}

/// A sphere of radius 1 at the origin, about 18 pixels across its radius from the default camera.
fn sphere(material: Value) -> Value {
    let mut params = json!({ "shape": "sphere", "size": [2, 2, 2] });
    for (name, value) in material.as_object().expect("material") {
        params[name] = value.clone();
    }
    json!({ "id": 1, "type": "shape_3d", "params": params })
}

fn lit_by(map: &str, params: Value) -> Value {
    let mut render = json!({ "environment_map": map });
    for (name, value) in params.as_object().expect("params") {
        render[name] = value.clone();
    }
    render
}

/// Pixels within `radius` pixels of the centre.
fn disc(radius: f32) -> impl Iterator<Item = (u32, u32)> {
    every_pixel().filter(move |&(x, y)| {
        let (dx, dy) = (
            x as f32 + 0.5 - CENTER as f32,
            y as f32 + 0.5 - CENTER as f32,
        );
        dx.hypot(dy) <= radius
    })
}

#[test]
fn a_sphere_in_a_uniform_environment_reflects_no_more_light_than_it_receives() {
    let media = TestMedia::default()
        .environment("grey.hdr", environment_file(64, 32, |_| Vec3::splat(0.5)));
    let expected = srgb8(0.5);
    for (metallic, roughness) in [(0, 1.0), (0, 0.3), (1, 0.0)] {
        let material = json!({ "metallic": metallic, "roughness": roughness });
        let frame = frame_of(
            render_3d_with(lit_by("grey.hdr", json!({})), vec![sphere(material)]),
            &media,
        );
        // Out to where the surface is seen within about 20° of edge on.
        for (x, y) in disc(16.5) {
            let [r, g, b, a] = frame.pixel(x, y);
            assert_eq!(a, 255);
            assert!(
                [r, g, b].iter().all(|c| c.abs_diff(expected) <= 3),
                "metallic {metallic}, roughness {roughness}: ({x}, {y}) is {r} {g} {b}, not \
                 {expected}"
            );
        }
    }
    // A rough metal loses the light its microfacets shadow, as it does under the lights.
    let frame = frame_of(
        render_3d_with(
            lit_by("grey.hdr", json!({})),
            vec![sphere(json!({ "metallic": 1, "roughness": 1 }))],
        ),
        &media,
    );
    let [r, ..] = frame.pixel(CENTER, CENTER);
    assert!(r < expected - 20, "a rough metal is darker, got {r}");
}

#[test]
fn a_diffuse_sphere_is_lit_from_where_the_environment_is_bright() {
    let media = TestMedia::default()
        .environment(
            "east.hdr",
            environment_file(64, 32, |d| Vec3::splat(f32::from(u8::from(d.x > 0.0)))),
        )
        .environment(
            "sky.hdr",
            environment_file(64, 32, |d| Vec3::splat(f32::from(u8::from(d.y > 0.0)))),
        );
    let diffuse = || vec![sphere(json!({ "roughness": 1 }))];
    // Green in linear light.
    let luma = |frame: &Frame, x: u32, y: u32| (f32::from(frame.pixel(x, y)[1]) / 255.0).powf(2.2);
    let east = frame_of(
        render_3d_with(lit_by("east.hdr", json!({})), diffuse()),
        &media,
    );
    assert!(luma(&east, CENTER + 12, CENTER) > 3.0 * luma(&east, CENTER - 12, CENTER));
    // Turned half way round, the bright side is to the west.
    let west = frame_of(
        render_3d_with(
            lit_by("east.hdr", json!({ "environment_rotation": 180 })),
            diffuse(),
        ),
        &media,
    );
    assert!(luma(&west, CENTER - 12, CENTER) > 3.0 * luma(&west, CENTER + 12, CENTER));
    let sky = frame_of(
        render_3d_with(lit_by("sky.hdr", json!({})), diffuse()),
        &media,
    );
    assert!(luma(&sky, CENTER, CENTER - 12) > 3.0 * luma(&sky, CENTER, CENTER + 12));
}

/// Grey, with a green patch behind the default camera at +Z, red to the west at -X, blue to
/// the east at +X, and white straight ahead at -Z, within the camera's view.
fn patches() -> TestMedia {
    TestMedia::default().environment(
        "patches.hdr",
        environment_file(128, 64, |d| {
            if d.z > 0.9 {
                Vec3::new(0.0, 0.8, 0.0)
            } else if d.x < -0.9 {
                Vec3::new(0.8, 0.0, 0.0)
            } else if d.x > 0.9 {
                Vec3::new(0.0, 0.0, 0.8)
            } else if d.z < -0.97 {
                Vec3::splat(0.8)
            } else {
                Vec3::splat(0.1)
            }
        }),
    )
}

fn mirror(roughness: f64) -> Value {
    sphere(json!({ "metallic": 1, "roughness": roughness }))
}

#[test]
fn a_mirror_reflects_the_environment_from_the_right_direction_as_it_turns() {
    let media = patches();
    let green = [0, srgb8(0.8), 0, 255];
    let red = [srgb8(0.8), 0, 0, 255];
    let blue = [0, 0, srgb8(0.8), 255];
    let mirror_with =
        |params: Value| render_3d_with(lit_by("patches.hdr", params), vec![mirror(0.0)]);
    // Where the surface faces 45° to the west, it reflects the view to the west.
    let west = (CENTER - 13, CENTER);
    check(vec![
        Case::new(
            "behind the camera at the centre, and west to the west",
            mirror_with(json!({})),
            Expect::Probes(vec![(CENTER, CENTER, green), (west.0, west.1, red)]),
        )
        .media(media.clone())
        .tolerance(24),
        Case::new(
            "a quarter turn brings the west behind the camera",
            mirror_with(json!({ "environment_rotation": 90 })),
            Expect::Probes(vec![(CENTER, CENTER, red)]),
        )
        .media(media.clone())
        .tolerance(24),
        Case::new(
            "and a quarter turn back brings the east there",
            mirror_with(json!({ "environment_rotation": -90 })),
            Expect::Probes(vec![(CENTER, CENTER, blue)]),
        )
        .media(media.clone())
        .tolerance(24),
    ]);

    let smooth = frame_of(mirror_with(json!({})), &media).pixel(CENTER, CENTER);
    let rough = frame_of(
        render_3d_with(lit_by("patches.hdr", json!({})), vec![mirror(0.7)]),
        &media,
    )
    .pixel(CENTER, CENTER);
    assert!(
        u32::from(rough[1]) * 10 < u32::from(smooth[1]) * 8 && rough[0] > smooth[0] + 20,
        "a rough mirror blurs the patch into its surroundings: {rough:?} vs {smooth:?}"
    );
}

#[test]
fn the_background_shows_the_environment_opaquely_behind_the_scene() {
    let media = patches();
    let camera = || json!({ "id": 2, "type": "camera_3d" });
    let white = srgb8(0.8);
    let grey = srgb8(0.1);
    let background = |params: Value, nodes: Vec<Value>| {
        let mut params = params;
        params["environment_background"] = json!(true);
        render_3d_with(params, nodes)
    };
    let opaque = |frame: &Frame| every_pixel().all(|(x, y)| frame.pixel(x, y)[3] == 255);

    let empty = frame_of(
        background(lit_by("patches.hdr", json!({})), vec![camera()]),
        &media,
    );
    assert!(
        opaque(&empty),
        "nothing in the scene still shows the background"
    );
    assert!(
        empty.pixel(CENTER, CENTER)[..3]
            .iter()
            .all(|c| c.abs_diff(white) <= 2),
        "ahead is -Z"
    );
    assert!(
        empty.pixel(0, CENTER)[..3]
            .iter()
            .all(|c| c.abs_diff(grey) <= 2)
    );

    let brighter = frame_of(
        background(
            lit_by("patches.hdr", json!({ "exposure": 1 })),
            vec![camera()],
        ),
        &media,
    );
    let doubled = srgb8(0.2);
    assert!(
        brighter.pixel(0, CENTER)[..3]
            .iter()
            .all(|c| c.abs_diff(doubled) <= 2)
    );
    let blurred = frame_of(
        background(
            lit_by("patches.hdr", json!({ "environment_background_blur": 1 })),
            vec![camera()],
        ),
        &media,
    );
    assert!(
        blurred.pixel(CENTER, CENTER)[0] < white - 40,
        "blurred into the grey around"
    );

    let behind = frame_of(
        background(lit_by("patches.hdr", json!({})), vec![mirror(0.0)]),
        &media,
    );
    assert!(opaque(&behind));
    assert!(
        behind.pixel(CENTER, CENTER)[1].abs_diff(srgb8(0.8)) <= 3,
        "the sphere, reflecting green behind the camera, is in front"
    );

    let transparent = |frame: &Frame| every_pixel().all(|(x, y)| frame.pixel(x, y)[3] == 0);
    let hidden = frame_of(
        render_3d_with(lit_by("patches.hdr", json!({})), vec![camera()]),
        &media,
    );
    assert!(
        transparent(&hidden),
        "without the background, nothing shows"
    );
    let without_map = frame_of(background(json!({}), vec![camera()]), &media);
    assert!(
        transparent(&without_map),
        "there is no background without a map"
    );
}

#[test]
fn a_shadow_catcher_darkens_the_background_by_the_light_its_shadow_takes_away() {
    let view = GroundView::WHOLE;
    let media = TestMedia::default()
        .environment("grey.hdr", environment_file(64, 32, |_| Vec3::splat(0.5)));
    let objects = vec![
        json!({ "id": 1, "type": "shape_3d", "params": { "size": [1, 0.2, 1], "position": [0, 1, 0] } }),
        json!({ "id": 2, "type": "shape_3d", "params": {
            "shape": "plane", "size": [6, 1, 6], "shadow_catcher": true, "position": [0, 0.01, 0] } }),
        light(
            60,
            json!({ "light": "directional", "intensity": 1, "rotation": [-90, 0, 0],
                    "cast_shadows": true }),
        ),
    ];
    let render = |background: bool| {
        lit_by(
            "grey.hdr",
            json!({ "environment": 1, "environment_background": background }),
        )
    };
    let (middle_x, middle_y) = view.pixel(0.0, 0.0);
    let (edge_x, edge_y) = view.pixel(2.5, 2.5);
    // The ground receives π·0.5 from all around, and 1 from the light the slab blocks.
    let around = std::f32::consts::PI * 0.5;
    let shadow = 1.0 - around / (around + 1.0);
    let shaded = srgb8(0.5 * (1.0 - shadow));
    check(vec![
        Case::new(
            "darkened in the shadow, and the background elsewhere",
            view.scene(objects.clone(), render(true)),
            Expect::Probes(vec![
                (middle_x, middle_y, [shaded, shaded, shaded, 255]),
                (edge_x, edge_y, [srgb8(0.5), srgb8(0.5), srgb8(0.5), 255]),
            ]),
        )
        .media(media.clone())
        .tolerance(3),
        Case::new(
            "and only the shadow without it",
            view.scene(objects, render(false)),
            Expect::Probes(vec![
                (
                    middle_x,
                    middle_y,
                    [0, 0, 0, (shadow * 255.0).round() as u8],
                ),
                (edge_x, edge_y, CLEAR),
            ]),
        )
        .media(media)
        .tolerance(3),
    ]);
}

#[test]
fn malformed_and_oversized_environment_maps_are_errors_and_unusable_texels_dark() {
    let bad = |bytes: &[u8]| {
        let media = TestMedia::default().environment("map.hdr", bytes.to_vec());
        gpu()
            .renderer()
            .render_frame_at(
                &render_3d_with(lit_by("map.hdr", json!({})), vec![sphere(json!({}))]).into(),
                0.0,
                &media,
            )
            .expect_err("the render fails")
            .to_string()
    };
    let mut truncated = environment_file(16, 8, |_| Vec3::ONE);
    truncated.truncate(truncated.len() - 10);
    let error = bad(&truncated);
    assert!(error.contains("environment map `map.hdr`"), "{error}");
    let error = bad(b"#?RADIANCE\n\n-Y 10 +X 9000\n");
    assert!(error.contains("larger than"), "{error}");
    assert!(bad(b"not a map").contains("map.hdr"));

    // Not-a-number, infinite and negative texels among grey ones light nothing.
    let mut bytes = std::io::Cursor::new(Vec::new());
    exr::prelude::Image::from_channels(
        (16, 8),
        exr::prelude::SpecificChannels::rgb(|at: exr::prelude::Vec2<usize>| match at.x() % 4 {
            0 if at.y() == 3 => (f32::NAN, f32::INFINITY, -100.0),
            _ => (0.5_f32, 0.5_f32, 0.5_f32),
        }),
    )
    .write()
    .to_buffered(&mut bytes)
    .expect("writes");
    let media = TestMedia::default().environment("odd.exr", bytes.into_inner());
    let frame = frame_of(
        render_3d_with(
            lit_by("odd.exr", json!({})),
            vec![sphere(json!({ "roughness": 1 }))],
        ),
        &media,
    );
    let [r, g, b, a] = frame.pixel(CENTER, CENTER);
    assert_eq!(a, 255);
    assert!(
        [r, g, b].iter().all(|&c| c > 150 && c <= srgb8(0.5) + 3),
        "{r} {g} {b}"
    );
}

#[test]
fn an_environment_lit_render_is_kept_and_turning_its_map_redraws_without_reloading_it() {
    let gpu = gpu();
    let frames = |rotation: Value| {
        let media = patches();
        let composition: Composition = render_3d_with(
            lit_by(
                "patches.hdr",
                json!({ "environment_rotation": rotation, "environment_background": true }),
            ),
            vec![mirror(0.0)],
        )
        .frames(4)
        .fps(4.0)
        .into();
        let mut renderer = gpu.renderer();
        let frames: Vec<Vec<u8>> = (0..4)
            .map(|frame| render(&mut renderer, &composition, f64::from(frame), &media).rgba)
            .collect();
        assert_eq!(media.environment_loads(), 1, "the map is read once");
        (frames, renderer.skipped_pass_count())
    };
    let (still, skipped) = frames(json!(30));
    assert!(still.iter().all(|frame| *frame == still[0]));
    assert!(
        skipped >= 3 * 2,
        "frames after the first read the kept draw and resolve, skipping {skipped} passes"
    );
    let (turning, skipped) = frames(json!("=time * 90"));
    assert_eq!(skipped, 0, "a turning map renders every frame");
    for pair in turning.windows(2) {
        assert!(pair[0] != pair[1], "the reflection turns");
    }
}
