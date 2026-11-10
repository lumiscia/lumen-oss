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
    render_3d_with(json!({}), scene_nodes)
}

fn render_3d_with(params: Value, scene_nodes: Vec<Value>) -> Scene {
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

fn merge(id: u32, objects: &[u32], params: Value) -> Value {
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
fn ortho(id: u32, params: Value) -> Value {
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
        merge(2, &[1], json!({})),
    ];
    let composition = Composition::from(render_3d(std::mem::take(&mut nodes)));
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    renderer.compile(&composition).expect("the scene compiles");
    let compiled = renderer.created_program_count();
    assert!(compiled > 0);

    render(&mut renderer, &composition, 0.0, &TestMedia::default());

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
    for &([left, bottom, right, top], material) in quads {
        let positions = [
            left, top, 0.0, right, top, 0.0, left, bottom, 0.0, right, bottom, 0.0,
        ];
        let position = accessor(floats(&positions), "VEC3", 5126, 4);
        let normal = accessor(floats(&[0.0, 0.0, 1.0].repeat(4)), "VEC3", 5126, 4);
        let uv = accessor(
            floats(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0]),
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
            "material": material,
        }));
    }
    let document = json!({
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

fn model(id: u32, params: Value) -> Value {
    json!({ "id": id, "type": "model_3d", "params": params })
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

#[test]
fn a_skinned_model_playing_a_clip_poses_anew_each_frame_and_a_still_one_is_kept() {
    let gpu = gpu();
    let frames = |animation: &str| {
        let composition: Composition = render_3d(vec![fox(1, json!({ "animation": animation }))])
            .frames(4)
            .fps(4.0)
            .into();
        let media = fox_media();
        let mut renderer = gpu.renderer();
        let frames: Vec<Vec<u8>> = (0..4)
            .map(|frame| render(&mut renderer, &composition, f64::from(frame), &media).rgba)
            .collect();
        assert_eq!(media.model_loads(), 1, "the model loads once");
        (frames, renderer.skipped_pass_count())
    };

    let (still, skipped) = frames("");
    assert!(still.iter().all(|frame| *frame == still[0]));
    assert!(
        skipped >= 3 * 2,
        "frames after the first read the kept draw and resolve, skipping {skipped} passes"
    );

    let (walking, skipped) = frames("Walk");
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
