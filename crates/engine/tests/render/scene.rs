//! `render_3d`: where the camera puts objects, what hides what, and when its output is kept.

use lumen_3d::glam::Vec3;
use lumen_engine::{
    composition::Composition, gpu::RenderTarget, media::collect_frame_requirements,
};
use serde_json::{Value, json};

use crate::support::scene::{
    CENTER, SIZE, covered, environment_file, flat, fox, fox_media, frame_of, framed, image, listed,
    lit_by, material, merge, model, plane, plate, quads, render_3d, render_3d_with, sphere, unlit,
    with_material,
};
use crate::support::{
    BLUE, CLEAR, Case, Expect, Frame, GREEN, RED, Scene, TestMedia, check, gpu, render, scene,
};

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
        11,
        json!({ "size": [2, 2, 0.2], "position": [0, 0, -1] }),
        json!({ "color": RED }),
    );
    let near = flat(
        2,
        12,
        json!({ "size": [0.5, 0.5, 0.2], "position": [0, 0, 1] }),
        json!({ "color": GREEN }),
    );
    let probes = || Expect::Probes(vec![(CENTER, CENTER, GREEN), (CENTER, 18, RED)]);
    check(vec![
        Case::new(
            "far node first",
            render_3d(listed(
                &[far.clone(), near.clone()],
                merge(3, &[1, 2], json!({})),
            )),
            probes(),
        ),
        Case::new(
            "near node first",
            render_3d(listed(&[far, near], merge(3, &[2, 1], json!({})))),
            probes(),
        ),
    ]);
}

#[test]
fn a_blended_object_shows_what_is_behind_it() {
    let half_green = flat(
        1,
        11,
        json!({ "size": [1, 1, 0.2], "position": [0.5, 0, 1] }),
        json!({ "color": GREEN, "opacity": 0.5 }),
    );
    let red = flat(
        2,
        12,
        json!({ "size": [1.5, 1, 0.2], "position": [-0.25, 0, -1] }),
        json!({ "color": RED }),
    );
    // The two overlap just right of centre. Half of each in linear light is sRGB 188.
    check(vec![
        Case::new(
            "the blended box is listed first and still blends over the opaque one",
            render_3d(listed(&[half_green, red], merge(3, &[1, 2], json!({})))),
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
        11,
        json!({ "size": [1, 1, 0.2], "position": [0.4, 0, 1] }),
        json!({ "color": GREEN, "opacity": 0.5 }),
    );
    let far = flat(
        2,
        12,
        json!({ "size": [1, 1, 0.2], "position": [-0.4, 0, -1] }),
        json!({ "color": RED, "opacity": 0.5 }),
    );
    // Half green over half red, in linear light: green 0.5, red 0.25, alpha 0.75. Red over green
    // would swap the red and green channels.
    let overlap = || Expect::Probes(vec![(CENTER, CENTER, [117, 160, 0, 191])]);
    check(vec![
        Case::new(
            "near node first",
            render_3d(listed(
                &[near.clone(), far.clone()],
                merge(3, &[1, 2], json!({})),
            )),
            overlap(),
        )
        .tolerance(2),
        Case::new(
            "far node first",
            render_3d(listed(&[near, far], merge(3, &[2, 1], json!({})))),
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
                vec![
                    material(9, json!({ "double_sided": true })),
                    with_material(facing_plane(json!({ "scale": [-1, 1, 1] })), 9),
                ],
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
            id + 10,
            json!({ "shape": "plane", "rotation": [90, 0, 0], "position": position }),
            json!({ "color": GREEN }),
        )
    };
    let with_camera = |camera: Value| {
        let mut camera = camera;
        camera["position"] = json!([0, 0, 5]);
        let mut nodes = square(1, [-1.0, 0.0, 0.0]).to_vec();
        nodes.extend(square(2, [1.0, 0.0, -4.0]));
        nodes.extend([
            json!({ "id": 3, "type": "camera_3d", "params": camera }),
            merge(4, &[1, 2, 3], json!({})),
        ]);
        render_3d(nodes)
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
fn a_merge_3d_that_is_not_visible_leaves_the_scene_with_its_camera() {
    let green = || {
        flat(
            2,
            12,
            json!({ "position": [1, 0, 0] }),
            json!({ "color": GREEN }),
        )
    };
    // A red box and a camera above the scene, in a merge beside the green box.
    let scene = |visible: Value| {
        let red = flat(
            1,
            11,
            json!({ "position": [-1, 0, 0] }),
            json!({ "color": RED }),
        );
        let camera = json!({ "id": 3, "type": "camera_3d",
            "params": { "position": [0, 3, 5], "look_at": true } });
        let mut nodes = listed(&[red, green()], camera);
        nodes.push(merge(4, &[1, 3], json!({ "visible": visible })));
        nodes.push(merge(5, &[4, 2], json!({})));
        render_3d(nodes).frames(2)
    };
    let green_alone = || Expect::Matches(render_3d(green().to_vec()).frames(2).into());
    check(vec![
        Case::new("hidden", scene(json!(false)), green_alone()),
        Case::new(
            "hidden from frame 1",
            scene(json!("=frame < 1")),
            Expect::PerFrame(vec![
                Expect::Matches(scene(json!(true)).into()),
                green_alone(),
            ]),
        )
        .frames([0.0, 1.0]),
    ]);
}

#[test]
fn a_merge_3d_that_is_not_visible_reads_nothing_wired_into_it() {
    // A plane showing an image, in a merge that is hidden on frame 0.
    let mut nodes = vec![plate(10)];
    nodes.extend(framed(10));
    nodes.push(merge(4, &[3], json!({ "visible": "=frame >= 1" })));
    let composition: Composition = render_3d(nodes).frames(2).into();

    let media = solid_plate(TestMedia::default(), "red", RED);
    let images = |frame| {
        collect_frame_requirements(&composition, &media, frame)
            .unwrap()
            .images
    };
    assert!(images(0).is_empty(), "nothing is decoded while hidden");
    assert_eq!(images(1), ["plate"]);

    // Without the image, only the frame that reaches the plane fails.
    let render = |frame| {
        let rendered = gpu().renderer().render(
            &composition,
            frame,
            &TestMedia::default(),
            RenderTarget::Owned,
        );
        rendered.map(|_| ()).map_err(|error| error.to_string())
    };
    assert_eq!(render(0.0), Ok(()));
    assert!(render(1.0).is_err(), "the shown plane reads its image");
}

#[test]
fn rotation_yaws_then_pitches_then_rolls() {
    // A bar along one axis shows which way the rotation turned it: up the image, across it, or
    // end on towards the camera.
    let bar = |size: [f64; 3], rotation: [f64; 3]| {
        render_3d(
            flat(
                1,
                11,
                json!({ "size": size, "rotation": rotation }),
                json!({ "color": GREEN }),
            )
            .to_vec(),
        )
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

/// Two unlit boxes, a unit either side of the centre, wired to one `material_3d` of `color`.
fn shared_material(color: Value) -> Scene {
    let box_at = |id: u32, x: f64| {
        with_material(
            json!({ "id": id, "type": "shape_3d", "params": { "position": [x, 0, 0] } }),
            10,
        )
    };
    render_3d(vec![
        material(10, json!({ "color": color, "unlit": true })),
        box_at(1, -1.0),
        box_at(2, 1.0),
        merge(3, &[1, 2], json!({})),
    ])
}

#[test]
fn one_material_3d_colours_every_object_it_is_wired_to() {
    // One unit at the origin is about 17.6 pixels, so the boxes are centred on columns 14 and 49.
    let both = |color| Expect::Probes(vec![(14, CENTER, color), (49, CENTER, color)]);
    check(vec![
        Case::new("red", shared_material(json!(RED)), both(RED)),
        Case::new(
            "changed to green",
            shared_material(json!(GREEN)),
            both(GREEN),
        ),
    ]);
}

#[test]
fn a_sphere_wraps_its_materials_texture_from_minus_z_with_its_top_row_at_the_pole() {
    // Blue in the top three eighths of rows, then red in the left half and green in the right.
    let media = image(|x, y| match (y < 24, x < CENTER) {
        (true, _) => BLUE,
        (false, true) => RED,
        (false, false) => GREEN,
    });
    let mut nodes = sphere(json!({ "unlit": true, "sampling": "nearest" }));
    nodes.insert(0, plate(10));
    nodes[1]["in"] = json!({ "texture": 10 });
    let frame = frame_of(render_3d(nodes), &media);
    // The camera on +Z sees the texture's middle: the left half of its columns to the left.
    assert_eq!(frame.pixel(CENTER - 9, CENTER), RED, "left of the middle");
    assert_eq!(
        frame.pixel(CENTER + 9, CENTER),
        GREEN,
        "right of the middle"
    );
    assert_eq!(frame.pixel(CENTER, CENTER - 15), BLUE, "near the top pole");
}

#[test]
fn a_material_3d_that_reads_the_frame_redraws_the_scene_and_a_static_one_keeps_it() {
    let gpu = gpu();
    let media = TestMedia::default();
    let frames = |color: Value| {
        let mut composition = shared_material(color).frames(4);
        composition.0["timeline"]["fps"] = json!(4);
        let composition = Composition::from(composition);
        let mut renderer = gpu.renderer();
        let frames: Vec<Frame> = (0..4)
            .map(|frame| render(&mut renderer, &composition, f64::from(frame), &media))
            .collect();
        (frames, renderer.skipped_pass_count())
    };

    let (still, skipped) = frames(json!(RED));
    assert!(still.iter().all(|frame| frame.rgba == still[0].rgba));
    assert!(
        skipped >= 3 * 2,
        "frames after the first read the kept draw and resolve, skipping {skipped} passes"
    );

    let (moving, skipped) = frames(json!("=rgb(frame * 40, 0, 0)"));
    assert_eq!(skipped, 0, "a material that animates renders every frame");
    for pair in moving.windows(2) {
        assert!(
            pair[0].pixel(14, CENTER) != pair[1].pixel(14, CENTER)
                && pair[0].pixel(49, CENTER) != pair[1].pixel(49, CENTER),
            "both boxes' colour changes"
        );
    }
}

fn render_error(composition: &Composition) -> String {
    gpu()
        .renderer()
        .render(composition, 0.0, &TestMedia::default(), RenderTarget::Owned)
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

/// The fox's 14 frames at 8 a second, its walk played once with `params` and not looped. The walk
/// lasts 0.83 seconds, so it has ended from the eighth frame, at 0.875.
fn walking_fox_frames(
    gpu: &crate::support::Gpu,
    reuse: bool,
    params: Value,
) -> (Vec<Frame>, Vec<usize>, u64) {
    let mut params = params;
    params["animation"] = json!("Walk");
    params["animation_loop"] = json!(false);
    let composition: Composition = render_3d(vec![fox(1, params)]).frames(14).fps(8.0).into();
    let media = fox_media();
    let mut renderer = if reuse {
        gpu.renderer()
    } else {
        gpu.reference()
    };
    let (mut frames, mut passes) = (Vec::new(), Vec::new());
    for frame in 0..14 {
        frames.push(render(
            &mut renderer,
            &composition,
            f64::from(frame),
            &media,
        ));
        passes.push(renderer.recorded_pass_count());
    }
    (frames, passes, renderer.skipped_pass_count())
}

#[test]
fn a_clip_that_has_ended_is_kept_as_its_pose_no_longer_changes() {
    let gpu = gpu();
    let (frames, passes, skipped) = walking_fox_frames(&gpu, true, json!({}));
    let (redrawn, _, _) = walking_fox_frames(&gpu, false, json!({}));
    for (kept, redrawn) in frames.iter().zip(&redrawn) {
        assert!(
            kept.rgba == redrawn.rgba,
            "frame {} differs when kept",
            kept.at
        );
    }
    assert!(frames[3].rgba != frames[4].rgba, "the fox walks");
    for frame in 9..14 {
        assert!(
            frames[frame].rgba == frames[8].rgba,
            "the pose holds at frame {frame}"
        );
        assert!(passes[frame] == 0, "frame {frame} records {passes:?}");
    }
    assert!(skipped > 0);
}

#[test]
fn a_clip_that_has_not_begun_is_kept_until_it_does() {
    let gpu = gpu();
    // The clip starts at a second.
    let params = json!({ "animation_offset": -1.0 });
    let (frames, passes, skipped) = walking_fox_frames(&gpu, true, params.clone());
    let (redrawn, _, _) = walking_fox_frames(&gpu, false, params);
    for (kept, redrawn) in frames.iter().zip(&redrawn) {
        assert!(
            kept.rgba == redrawn.rgba,
            "frame {} differs when kept",
            kept.at
        );
    }
    for frame in 1..8 {
        assert!(
            frames[frame].rgba == frames[0].rgba,
            "the pose holds at frame {frame}"
        );
        assert!(passes[frame] == 0, "frame {frame} records {passes:?}");
    }
    assert!(
        frames[11].rgba != frames[0].rgba,
        "the fox walks once the clip begins"
    );
    assert!(skipped > 0);
}

/// Renders frames 0 and 1 of `composition` with `before`, so that frame 1 is kept, then frame 2
/// with `after`, which serves one of its sources as other bytes under a new identity. Frame 2
/// must show them as a renderer that never saw `before` does.
fn kept_then_replaced(composition: &Composition, before: &TestMedia, after: &TestMedia) {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let first = render(&mut renderer, composition, 0.0, before);
    assert!(render(&mut renderer, composition, 1.0, before).rgba == first.rgba);
    assert!(
        renderer.skipped_pass_count() > 0,
        "the unchanged frame is kept"
    );
    let replaced = render(&mut renderer, composition, 2.0, after);
    assert!(replaced.rgba != first.rgba, "the new bytes show");
    let expected = render(&mut gpu.reference(), composition, 2.0, after);
    assert!(replaced.rgba == expected.rgba);
}

#[test]
fn an_environment_kept_as_a_static_frame_is_drawn_again_when_its_map_changes() {
    let composition: Composition = render_3d_with(
        lit_by("sky.hdr", json!({ "environment_background": true })),
        sphere(json!({})),
    )
    .frames(3)
    .into();
    let map = |level: f32| environment_file(16, 8, move |_| Vec3::splat(level));
    let dim = TestMedia::default().environment_with_id("sky.hdr", "dim", map(0.2));
    let bright = TestMedia::default().environment_with_id("sky.hdr", "bright", map(0.8));
    kept_then_replaced(&composition, &dim, &bright);
}

fn solid_plate(media: TestMedia, id: &str, color: [u8; 4]) -> TestMedia {
    media.image_with_id("plate", id, 4, 4, color.repeat(16))
}

#[test]
fn a_planes_image_kept_as_a_static_frame_is_drawn_again_when_it_changes() {
    let mut nodes = vec![plate(10)];
    nodes.extend(framed(10));
    let composition: Composition = render_3d(nodes).frames(3).into();
    let red = solid_plate(TestMedia::default(), "red", RED);
    let green = solid_plate(TestMedia::default(), "green", GREEN);
    kept_then_replaced(&composition, &red, &green);
}

#[test]
fn a_planes_image_kept_while_a_model_holds_its_pose_is_drawn_again_when_it_changes() {
    let walk = json!({ "animation": "Walk", "animation_loop": false });
    let nodes = vec![
        fox(1, walk),
        plate(10),
        plane(11, 10, json!({ "width": 1, "position": [0, 0, 1] })),
        merge(12, &[1, 11], json!({})),
    ];
    let composition: Composition = render_3d(nodes).frames(14).fps(8.0).into();
    let red = solid_plate(fox_media(), "red", RED);
    let green = solid_plate(fox_media(), "green", GREEN);
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    // The walk ends by frame 9, after which the pose holds and the scene is kept.
    for frame in 0..12 {
        render(&mut renderer, &composition, f64::from(frame), &red);
    }
    let held = render(&mut renderer, &composition, 12.0, &red);
    assert_eq!(renderer.recorded_pass_count(), 0, "the held pose is kept");
    let replaced = render(&mut renderer, &composition, 13.0, &green);
    assert!(replaced.rgba != held.rgba, "the new image shows");
    let expected = render(&mut gpu.reference(), &composition, 13.0, &green);
    assert!(replaced.rgba == expected.rgba);
}

#[test]
fn a_model_kept_as_a_static_frame_is_drawn_again_when_its_file_changes() {
    let gpu = gpu();
    let quad = |color: [f32; 4]| {
        quads(
            json!([unlit("card", color)]),
            &[([-1.0, -1.0, 1.0, 1.0], 0)],
        )
    };
    let composition: Composition = render_3d(vec![model(1, json!({ "source": "card.glb" }))])
        .frames(3)
        .into();
    let red = TestMedia::default().model_with_id("card.glb", "red", quad([1.0, 0.0, 0.0, 1.0]));
    let green = TestMedia::default().model_with_id("card.glb", "green", quad([0.0, 1.0, 0.0, 1.0]));
    let mut renderer = gpu.renderer();
    let center = |frame: &Frame| frame.pixel(CENTER, CENTER);
    assert_eq!(center(&render(&mut renderer, &composition, 0.0, &red)), RED);
    assert_eq!(center(&render(&mut renderer, &composition, 1.0, &red)), RED);
    assert!(
        renderer.skipped_pass_count() > 0,
        "the unchanged model is kept"
    );
    assert_eq!(
        center(&render(&mut renderer, &composition, 2.0, &green)),
        GREEN
    );
}
