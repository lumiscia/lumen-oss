//! `render_3d`'s depth and normal output: the values it holds, what draws into it, and that it
//! shares what the colour render of the same scene loads.

use lumen_3d::glam::Vec3;
use lumen_engine::composition::Composition;
use serde_json::{Value, json};

use crate::scene::{Quad, SIZE, fox, fox_media, light, merge, model, model_glb, ortho, texel_png};
use crate::support::{Frame, Scene, TestMedia, gpu, render, scene};

const BACKGROUND: [u8; 4] = [255, 255, 255, 255];

/// `scene_nodes`, the last of which is the scene, drawn by a `render_3d` with `params`.
fn output(params: Value, scene_nodes: Vec<Value>) -> Scene {
    let last = scene_nodes.last().expect("a scene node")["id"].clone();
    let mut nodes = scene_nodes;
    nodes
        .push(json!({ "id": 100, "type": "render_3d", "params": params, "in": { "scene": last } }));
    scene(SIZE, json!(nodes))
}

fn frame(scene: Scene, media: &TestMedia) -> Frame {
    render(&mut gpu().renderer(), &scene.into(), 0.0, media)
}

/// A plane facing the default camera, `size` wide and tall, at `position`.
fn facing(id: u32, position: [f64; 3], size: [f64; 2], params: Value) -> Value {
    let mut node = json!({
        "shape": "plane",
        "size": [size[0], 1, size[1]],
        "rotation": [90, 0, 0],
        "position": position,
    });
    for (name, value) in params.as_object().expect("params") {
        node[name] = value.clone();
    }
    json!({ "id": id, "type": "shape_3d", "params": node })
}

/// A plane 5 units from the default camera on the left, and one 8 units away on the right.
fn two_planes(camera: Option<Value>) -> Vec<Value> {
    let mut nodes = vec![
        facing(1, [-0.8, 0.0, 0.0], [1.4, 3.0], json!({})),
        facing(2, [0.8, 0.0, -3.0], [1.4, 3.0], json!({})),
    ];
    let mut objects = vec![1, 2];
    if let Some(camera) = camera {
        nodes.push(camera);
        objects.push(3);
    }
    nodes.push(merge(4, &objects, json!({})));
    nodes
}

/// Asserts `pixel` is opaque grey within a level of `depth` in 0..1.
fn grey(pixel: [u8; 4], depth: f32, what: &str) {
    let [r, g, b, a] = pixel;
    let expected = (depth * 255.0).round() as u8;
    assert!(
        r == g && g == b && a == 255 && r.abs_diff(expected) <= 1,
        "{what}: expected opaque grey {expected}, got {pixel:?}"
    );
}

#[test]
fn depth_is_the_distance_along_the_view_between_depth_near_and_depth_far() {
    let media = TestMedia::default();
    let ranged = json!({ "output": "depth", "depth_near": 3, "depth_far": 11 });
    let perspective = frame(output(ranged.clone(), two_planes(None)), &media);
    // The near plane, at its centre and towards its corner, where a distance from the eye would
    // be 6% greater: depth is along the view, so a plane facing the camera is one value.
    grey(perspective.pixel(18, 32), 2.0 / 8.0, "5 units away");
    grey(
        perspective.pixel(14, 10),
        2.0 / 8.0,
        "5 units away, off centre",
    );
    grey(perspective.pixel(41, 32), 5.0 / 8.0, "8 units away");
    assert_eq!(
        perspective.pixel(2, 2),
        BACKGROUND,
        "nothing drawn is as far as can be"
    );

    let ortho = json!({ "id": 3, "type": "camera_3d", "params": {
        "projection": "orthographic", "ortho_height": 4, "position": [0, 0, 5],
    } });
    let orthographic = frame(output(ranged, two_planes(Some(ortho))), &media);
    grey(
        orthographic.pixel(12, 32),
        2.0 / 8.0,
        "5 units away, orthographic",
    );
    grey(
        orthographic.pixel(48, 32),
        5.0 / 8.0,
        "8 units away, orthographic",
    );

    let clamped = frame(
        output(
            json!({ "output": "depth", "depth_near": 6, "depth_far": 7 }),
            two_planes(None),
        ),
        &media,
    );
    grey(clamped.pixel(18, 32), 0.0, "nearer than depth_near");
    grey(clamped.pixel(41, 32), 1.0, "farther than depth_far");

    // The default camera draws from 0.05 to 1000 units.
    let camera_range = frame(
        output(json!({ "output": "depth" }), two_planes(None)),
        &media,
    );
    grey(
        camera_range.pixel(18, 32),
        4.95 / 999.95,
        "the camera's range",
    );
    grey(
        camera_range.pixel(41, 32),
        7.95 / 999.95,
        "the camera's range",
    );

    let backwards = output(
        json!({ "output": "depth", "depth_near": 5, "depth_far": 2 }),
        two_planes(None),
    );
    let error = gpu()
        .renderer()
        .render_frame_at(&backwards.into(), 0.0, &media)
        .expect_err("a depth range that runs backwards fails");
    assert!(
        error
            .to_string()
            .contains("depth_far (2) must be farther than depth_near (5)"),
        "{error}"
    );

    // A distance that is negative, not a number or past what a float holds names its param,
    // on the frame it becomes one.
    for (params, frame, named) in [
        (
            json!({ "depth_near": -3, "depth_far": 11 }),
            0.0,
            "depth_near (-3)",
        ),
        (
            json!({ "depth_near": "=pow(-1, 0.5)", "depth_far": 11 }),
            0.0,
            "depth_near (NaN)",
        ),
        (
            json!({ "depth_near": 3, "depth_far": 1e40 }),
            0.0,
            "depth_far (1000000",
        ),
        (
            json!({ "depth_near": 3, "depth_far": "=11 - frame * 20" }),
            1.0,
            "depth_far (-9)",
        ),
    ] {
        let mut params = params;
        params["output"] = json!("depth");
        let composition = output(params, two_planes(None));
        let mut renderer = gpu().renderer();
        if frame > 0.0 {
            renderer
                .render_frame_at(&Scene(composition.0.clone()).into(), 0.0, &media)
                .expect("the range is good on the first frame");
        }
        let error = renderer
            .render_frame_at(&composition.into(), frame, &media)
            .expect_err("a distance that can't be one fails");
        assert!(error.to_string().contains(named), "{named}: {error}");
    }
}

#[test]
fn blended_surfaces_write_depth_only_where_their_alpha_reaches_half() {
    // A plane 4 units away over the left half of the frame, in front of a wall 8 units away.
    let wall = facing(2, [0.0, 0.0, -3.0], [8.0, 8.0], json!({}));
    let range = json!({ "output": "depth", "depth_near": 3, "depth_far": 11 });
    let left = (12, 32);
    let in_front = |params: Value| {
        let front = facing(1, [-1.0, 0.0, 1.0], [2.0, 4.0], params);
        frame(
            output(
                range.clone(),
                vec![front, wall.clone(), merge(3, &[1, 2], json!({}))],
            ),
            &TestMedia::default(),
        )
        .pixel(left.0, left.1)
    };
    grey(in_front(json!({})), 1.0 / 8.0, "an opaque plane");
    grey(
        in_front(json!({ "opacity": 0.6 })),
        1.0 / 8.0,
        "a plane at 0.6 opacity",
    );
    grey(
        in_front(json!({ "opacity": 0.4 })),
        5.0 / 8.0,
        "the wall behind a plane at 0.4",
    );

    // An image plane filling the frame 5 units away, whose source is opaque on its left half.
    let pixels = (0..SIZE.1)
        .flat_map(|_| (0..SIZE.0).map(|x| [0, 0, 255, if x < 32 { 255 } else { 0 }]))
        .flatten()
        .collect();
    let media = TestMedia::default().image("half", SIZE.0, SIZE.1, pixels);
    let nodes = vec![
        json!({ "id": 5, "type": "media_in", "params": { "kind": "image", "source": "half" } }),
        json!({ "id": 1, "type": "image_plane_3d", "params": { "width": 4 }, "in": { "source": 5 } }),
        wall,
        merge(3, &[1, 2], json!({})),
    ];
    let depth = frame(output(range, nodes), &media);
    grey(
        depth.pixel(20, 32),
        2.0 / 8.0,
        "the image plane's opaque half",
    );
    grey(
        depth.pixel(44, 32),
        5.0 / 8.0,
        "the wall through its transparent half",
    );
}

/// The unit normal of the sphere of radius 1 at the origin where the ray through the centre of
/// `pixel` from 5 units along the view's +Z, with the default 40° field of view, first meets it, in
/// the view's space.
fn sphere_normal((x, y): (u32, u32)) -> Option<Vec3> {
    let tan = 20f32.to_radians().tan();
    let ndc = |at: u32| (at as f32 + 0.5) / (SIZE.0 as f32 / 2.0) - 1.0;
    let direction = Vec3::new(ndc(x) * tan, -ndc(y) * tan, -1.0).normalize();
    let eye = Vec3::new(0.0, 0.0, 5.0);
    let b = eye.dot(direction);
    let disc = b * b - (eye.length_squared() - 1.0);
    (disc > 0.0).then(|| eye + direction * (-b - disc.sqrt()))
}

fn encoded(normal: Vec3) -> [u8; 3] {
    (normal * 0.5 + 0.5)
        .to_array()
        .map(|n| (n * 255.0).round() as u8)
}

#[test]
fn a_spheres_normals_match_its_analytic_normals_in_the_cameras_space() {
    let sphere =
        json!({ "id": 1, "type": "shape_3d", "params": { "shape": "sphere", "size": [2, 2, 2] } });
    // From the front, and from the side, which turns the world but not the view's normals.
    let cameras = [
        json!({ "id": 2, "type": "camera_3d", "params": {} }),
        json!({ "id": 2, "type": "camera_3d", "params": { "position": [5, 0, 0], "look_at": true } }),
    ];
    for camera in cameras {
        let normals = frame(
            output(
                json!({ "output": "normal" }),
                vec![sphere.clone(), camera.clone(), merge(3, &[1, 2], json!({}))],
            ),
            &TestMedia::default(),
        );
        let mut checked = 0;
        for (x, y) in (0..SIZE.1).flat_map(|y| (0..SIZE.0).map(move |x| (x, y))) {
            let pixel = normals.pixel(x, y);
            let Some(normal) = sphere_normal((x, y)) else {
                if sphere_normal((x + 1, y)).is_none()
                    && sphere_normal((x.saturating_sub(1), y)).is_none()
                {
                    assert_eq!(
                        pixel,
                        [0, 0, 0, 0],
                        "({x}, {y}) is off the sphere: {camera}"
                    );
                }
                continue;
            };
            // Off the silhouette, where the facets of the mesh stray little from the sphere.
            if normal.z < 0.5 {
                continue;
            }
            let expected = encoded(normal);
            assert_eq!(pixel[3], 255, "({x}, {y}) is on the sphere");
            assert!(
                (0..3).all(|c| pixel[c].abs_diff(expected[c]) <= 4),
                "({x}, {y}): expected {expected:?}, got {pixel:?} from {camera}"
            );
            checked += 1;
        }
        assert!(checked > 300, "{checked} pixels checked");
    }
}

#[test]
fn the_back_of_a_double_sided_surface_has_the_normal_it_is_lit_by() {
    let normal_of = |params: Value| {
        let plane = facing(1, [0.0, 0.0, 0.0], [3.0, 3.0], params);
        let frame = frame(
            output(json!({ "output": "normal" }), vec![plane]),
            &TestMedia::default(),
        );
        frame.pixel(32, 32)
    };
    // +Z, towards the camera, which is 127.5 in red and green.
    let towards = |pixel: [u8; 4]| {
        pixel[0].abs_diff(128) <= 1 && pixel[1].abs_diff(128) <= 1 && pixel[2..] == [255, 255]
    };
    let front = normal_of(json!({}));
    assert!(towards(front), "the front faces the camera: {front:?}");
    let back = normal_of(json!({ "rotation": [-90, 0, 0], "double_sided": true }));
    assert!(towards(back), "the back, reversed, faces it too: {back:?}");
}

#[test]
fn normal_output_is_the_normal_a_normal_map_tilts_the_surface_to() {
    let lean = std::f32::consts::FRAC_1_SQRT_2;
    // A card whose normal map leans it towards +X on the left, and one whose vertex normals lean
    // it the same way on the right.
    let quad = |left: f32, material: usize, normal: [f32; 3]| Quad {
        edges: [left, -0.6, left + 1.2, 0.6],
        material,
        normal,
        mirrored_u: false,
    };
    let glb = model_glb(
        json!([
            { "normalTexture": { "index": 0 } },
            {},
        ]),
        Some(&texel_png([218, 128, 218, 255])),
        &[
            quad(-1.5, 0, [0.0, 0.0, 1.0]),
            quad(0.3, 1, [lean, 0.0, lean]),
        ],
    );
    let nodes = vec![
        model(1, json!({ "source": "card.glb" })),
        ortho(2, json!({})),
        merge(3, &[1, 2], json!({})),
    ];
    let normals = frame(
        output(json!({ "output": "normal" }), nodes),
        &TestMedia::default().model("card.glb", glb),
    );
    let expected = encoded(Vec3::new(lean, 0.0, lean));
    for x in [14, 46] {
        let pixel = normals.pixel(x, 32);
        assert!(
            pixel[3] == 255 && (0..3).all(|c| pixel[c].abs_diff(expected[c]) <= 3),
            "({x}, 32): expected {expected:?}, got {pixel:?}"
        );
    }
}

/// A fox, a box and an image plane lit by a light casting shadows, coloured, and with its depth.
fn fog_nodes(masked: bool, render_depth: bool) -> Vec<Value> {
    let mut nodes = vec![
        fox(1, json!({ "animation": "Walk" })),
        json!({ "id": 2, "type": "shape_3d", "params": { "position": [-1, 0.5, -2] } }),
        json!({ "id": 3, "type": "media_in", "params": { "kind": "image", "source": "plate" } }),
        json!({ "id": 4, "type": "image_plane_3d", "params": { "position": [1, 0.5, -1] }, "in": { "source": 3 } }),
        light(5, json!({ "cast_shadows": true })),
        merge(6, &[1, 2, 4, 5], json!({})),
        json!({ "id": 7, "type": "render_3d", "in": { "scene": 6 } }),
        json!({ "id": 8, "type": "render_3d", "params": { "output": "depth", "depth_near": 3, "depth_far": 9 }, "in": { "scene": 6 } }),
        json!({ "id": 9, "type": "background", "params": { "paint": [200, 210, 220, 255] } }),
    ];
    let mut fog = json!({ "id": 10, "type": "merge", "params": { "mask_mode": "luma" }, "in": { "base": 7, "overlay": 9 } });
    if masked {
        fog["in"]["mask"] = json!(8);
    }
    if !render_depth {
        nodes.retain(|node| node["id"] != 8);
    }
    nodes.push(fog);
    nodes
}

#[test]
fn a_depth_render_of_a_scene_shares_its_loads_with_the_colour_render_and_skips_its_lighting() {
    let gpu = gpu();
    let media = || {
        let pixels = vec![[90, 160, 40, 255]; (SIZE.0 * SIZE.1) as usize].concat();
        fox_media().image("plate", SIZE.0, SIZE.1, pixels)
    };
    let passes = |nodes: Vec<Value>, media: &TestMedia| {
        let mut renderer = gpu.renderer();
        let composition: Composition = scene(SIZE, json!(nodes)).into();
        render(&mut renderer, &composition, 0.0, media);
        (
            renderer.submitted_pass_count(),
            renderer.media_texture_upload_count(),
        )
    };
    let fogged_media = media();
    let (fogged, uploads) = passes(fog_nodes(true, true), &fogged_media);
    assert_eq!(
        fogged_media.model_loads(),
        1,
        "both renders draw one load of the fox"
    );
    assert_eq!(
        uploads, 1,
        "both renders show one upload of the plane's source"
    );
    let (unfogged, _) = passes(fog_nodes(false, false), &media());

    // The fox, the box and the plane draw once each, then the depth is stored: no shadow map, no
    // mipmaps and no environment. The plane reads its source's texture as it is.
    let mut depth_alone = fog_nodes(true, true);
    depth_alone.retain(|node| ![7, 9, 10].contains(&node["id"].as_u64().unwrap()));
    let (depth_passes, _) = passes(depth_alone, &media());
    assert_eq!(
        depth_passes, 4,
        "the depth render records {depth_passes} passes"
    );
    assert_eq!(
        fogged - unfogged,
        depth_passes,
        "beside the colour render, the depth render records only its own passes"
    );
}

#[test]
fn color_output_ignores_the_depth_range() {
    let media = TestMedia::default();
    let color = frame(output(json!({}), two_planes(None)), &media);
    let explicit = frame(
        output(
            json!({ "output": "color", "depth_near": 6, "depth_far": 7 }),
            two_planes(None),
        ),
        &media,
    );
    assert!(
        color.rgba.iter().any(|&value| value != 0),
        "the planes show"
    );
    assert_eq!(color.rgba, explicit.rgba);
}
