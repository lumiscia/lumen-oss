//! An image plane among the other objects: the canvas of its clip, transparent texels, depth order and light.

use lumen_3d::glam::Vec3;
use lumen_engine::{composition::Composition, gpu::RenderTarget};
use serde_json::{Value, json};

use crate::support::scene::{
    CENTER, SIZE, covered, environment_file, flat, framed, half_blue, image, light, lit_by,
    material, merge, ortho, plane, plate, render_3d, render_3d_with, sphere, with_material,
};
use crate::support::{BLUE, CLEAR, Case, Expect, GREEN, RED, Scene, TestMedia, check, gpu, render};

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

#[test]
fn a_planes_transparent_texels_show_what_is_behind_without_hiding_it() {
    let behind = |plane_first: bool| {
        let mut objects: Vec<Value> = vec![
            plate(10),
            plane(1, 10, json!({ "width": 4 })),
            ortho(2, json!({})),
        ];
        objects.extend(flat(
            4,
            14,
            json!({ "size": [4, 4, 0.2], "position": [0, 0, -2] }),
            json!({ "color": RED }),
        ));
        objects.extend(flat(
            5,
            15,
            json!({ "size": [1, 1, 0.2], "position": [1, 0, -1] }),
            json!({ "color": GREEN, "opacity": 0.5 }),
        ));
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
    let mut nodes = vec![plate(10), plane(1, 10, params), ortho(2, json!({}))];
    nodes.extend(flat(
        3,
        13,
        json!({ "size": [3, 1, 0.1], "position": [0, 0, 0.5] }),
        json!({ "color": GREEN, "opacity": 0.5 }),
    ));
    nodes.push(merge(4, &[1, 2, 3], json!({})));
    render_3d(nodes)
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
    let framed_plane = |params: Value, material_params: Value| {
        let mut params = params;
        params["width"] = json!(4);
        let mut material_params = material_params;
        material_params["unlit"] = json!(true);
        material_params["double_sided"] = json!(true);
        render_3d(vec![
            plate(10),
            material(11, material_params),
            with_material(plane(1, 10, params), 11),
            ortho(2, json!({})),
            merge(3, &[1, 2], json!({})),
        ])
    };
    check(vec![
        Case::new(
            "blended, the transparent half shows nothing",
            framed_plane(json!({}), json!({})),
            Expect::Probes(vec![(16, CENTER, BLUE), (48, CENTER, CLEAR)]),
        )
        .media(half_blue()),
        Case::new(
            "opaque, the transparent half is opaque black and opacity is ignored",
            framed_plane(json!({ "opaque": true }), json!({ "opacity": 0.2 })),
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
            material(11, json!({ "double_sided": true, "roughness": 1 })),
            with_material(plane(1, 10, json!({ "width": 4 })), 11),
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
        material(6, json!({ "shadow_catcher": "=frame > 0" })),
        with_material(
            json!({ "id": 4, "type": "shape_3d", "params": {
                "shape": "plane", "size": [6, 1, 6], "position": [0, -1, 0] } }),
            6,
        ),
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
        sphere(json!({})),
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
        .render(&wide.into(), 0.0, &image(|_, _| RED), RenderTarget::Owned)
        .expect_err("the render fails")
        .to_string();
    assert!(
        error.contains(&format!(
            "a {width}x64 canvas is larger than this device's {max}-pixel texture limit"
        )) && error.contains("clip node"),
        "{error}"
    );
}

#[test]
fn masked_planes_that_cross_draw_in_depth_order_where_they_cross() {
    let masked = |id: u32, color: &str, params: Value| {
        [
            material(
                id + 10,
                json!({
                    "color": color, "unlit": true, "double_sided": true, "alpha_mode": "mask"
                }),
            ),
            with_material(plane(id, 10, params), id + 10),
        ]
    };
    // The second plane's right half turns away from the camera, behind the first.
    let mut nodes = vec![plate(10)];
    nodes.extend(masked(1, "#ff0000", json!({ "width": 4 })));
    nodes.extend(masked(
        4,
        "#0000ff",
        json!({ "width": 4, "rotation": [0, 60, 0] }),
    ));
    nodes.extend([ortho(2, json!({})), merge(3, &[1, 4, 2], json!({}))]);
    check(vec![
        Case::new(
            "each plane is in front on its own side of where they cross",
            render_3d_with(json!({ "supersample": 1 }), nodes),
            Expect::Probes(vec![(24, CENTER, BLUE), (40, CENTER, RED)]),
        )
        .media(image(|_, _| [255, 255, 255, 255])),
    ]);
}
