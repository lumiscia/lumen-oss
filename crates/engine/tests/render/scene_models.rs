//! `model_3d`: textures, materials, skins and the clips and materials a model must have.

use lumen_engine::{composition::Composition, gpu::RenderTarget};
use serde_json::{Value, json};

use crate::support::scene::{
    CENTER, Quad, SIZE, fox, fox_media, gradient, image, merge, model, model_glb, ortho, plane,
    plate, quads, render_3d, render_3d_with, texel_png, unlit,
};
use crate::support::{
    BLUE, CLEAR, Case, Expect, Frame, GREEN, RED, Scene, TestMedia, check, gpu, render, scene,
};

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
fn a_masked_cards_cut_out_texels_hide_nothing_drawn_after_them() {
    let media = || {
        let mut material = unlit("card", [0.0, 0.0, 1.0, 0.4]);
        material["alphaMode"] = json!("MASK");
        image(|_, _| RED).model(
            "card.glb",
            quads(json!([material]), &[([-3.0, -3.0, 3.0, 3.0], 0)]),
        )
    };
    let behind = |object: Value, order: &[u32]| {
        let mut nodes = vec![
            plate(10),
            model(2, json!({ "source": "card.glb" })),
            ortho(3, json!({})),
        ];
        nodes.push(object);
        nodes.push(merge(4, order, json!({})));
        render_3d(nodes)
    };
    let mut wall = crate::support::scene::flat(
        5,
        15,
        json!({ "size": [4, 4, 0.2], "position": [0, 0, -2] }),
        json!({ "color": RED }),
    )
    .to_vec();
    let wall_after = {
        let mut scene = behind(wall.pop().expect("wall"), &[2, 5, 3]);
        scene.0["nodes"].as_array_mut().expect("nodes").extend(wall);
        scene
    };
    let red_plane = plane(5, 10, json!({ "width": 4, "position": [0, 0, -2] }));
    check(vec![
        Case::new(
            "an opaque wall listed after",
            wall_after,
            Expect::Uniform(RED),
        )
        .media(media())
        .tolerance(1),
        Case::new(
            "a blended plane",
            behind(red_plane, &[2, 5, 3]),
            Expect::Uniform(RED),
        )
        .media(media())
        .tolerance(1),
    ]);
}

#[test]
fn a_clip_or_material_the_model_lacks_is_an_error_listing_those_it_has() {
    let error = |params: Value| {
        let composition: Composition = render_3d(vec![fox(1, params)]).into();
        gpu()
            .renderer()
            .render(&composition, 0.0, &fox_media(), RenderTarget::Owned)
            .expect_err("the render fails")
            .to_string()
    };
    assert_eq!(
        error(json!({ "animation": "Dance" })),
        "model_3d node 1 evaluation failed: model `fox.glb`: the model has no animation `Dance`; \
         it has 0 `Survey`, 1 `Walk`, 2 `Run`"
    );
    assert_eq!(
        error(json!({ "texture_material": "screen" })),
        "model_3d node 1 evaluation failed: model `fox.glb`: the model has no material `screen`; \
         it has `fox_material`"
    );
}
