//! What a `render_3d` reports of the scene it places on a frame, and drawing it through a camera
//! of the host's.

use lumen_3d::glam::{Mat4, Vec3};
use lumen_engine::{
    composition::Composition,
    gpu::{CameraProjection, GpuCompositionRenderer, RenderTarget, SceneCamera, SceneCapture},
    node::NodeId,
};
use serde_json::{Value, json};

use crate::support::scene::{SIZE, flat, listed, merge, render_3d};
use crate::support::{Scene, TestMedia, gpu, render, scene};

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];

fn camera(id: u32, params: Value) -> Value {
    let mut params = params;
    params["fov"] = json!(50);
    params["near"] = json!(0.1);
    params["far"] = json!(100);
    json!({ "id": id, "type": "camera_3d", "params": params })
}

/// A red box at x = -1 and a wider green one at x = 1, seen by `cameras`.
fn two_shapes(cameras: &[Value]) -> Scene {
    let mut ids = vec![1, 2];
    ids.extend(
        cameras
            .iter()
            .map(|camera| camera["id"].as_u64().unwrap() as u32),
    );
    let mut nodes = listed(
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
                json!({ "position": [1, 0, 0], "size": [2, 1, 1] }),
                json!({ "color": GREEN }),
            ),
        ],
        merge(9, &ids, json!({})),
    );
    nodes.splice(nodes.len() - 1..nodes.len() - 1, cameras.iter().cloned());
    render_3d(nodes)
}

/// Renders `frame`, which keeps the outputs it can for the next.
fn draw(renderer: &mut GpuCompositionRenderer, composition: &Composition, frame: f64) {
    renderer
        .render(
            composition,
            frame,
            &TestMedia::default(),
            RenderTarget::Owned,
        )
        .expect("frame renders");
}

fn near(a: &[f32], b: &[f32]) -> bool {
    a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-4)
}

fn box_world(position: Vec3, size: Vec3) -> [f32; 16] {
    Mat4::from_scale_rotation_translation(size, Default::default(), position).to_cols_array()
}

#[test]
fn a_capture_reports_each_object_with_its_node_world_and_bounds_and_the_scenes_cameras() {
    let composition: Composition = two_shapes(&[
        camera(3, json!({ "position": [0, 0, 5] })),
        camera(
            4,
            json!({ "position": [2, 3, 5], "projection": "orthographic", "ortho_height": 6 }),
        ),
    ])
    .into();
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    renderer.set_scene_capture(true);
    draw(&mut renderer, &composition, 0.0);

    let [capture] = renderer
        .take_scene_captures()
        .try_into()
        .expect("one capture");
    assert_eq!(
        (capture.render, capture.template.as_deref()),
        (NodeId::new(100), None)
    );
    let objects: Vec<_> = capture
        .objects
        .iter()
        .map(|object| {
            (
                object.node,
                object.world,
                object.bounds.min,
                object.bounds.max,
            )
        })
        .collect();
    let expected = [
        (1, Vec3::new(-1.0, 0.0, 0.0), Vec3::ONE),
        (2, Vec3::new(1.0, 0.0, 0.0), Vec3::new(2.0, 1.0, 1.0)),
    ];
    assert_eq!(objects.len(), expected.len());
    for ((node, world, min, max), (id, position, size)) in objects.iter().zip(expected) {
        assert_eq!(*node, NodeId::new(id));
        assert!(near(world, &box_world(position, size)), "{world:?}");
        assert!(near(min, &(position - size * 0.5).to_array()), "{min:?}");
        assert!(near(max, &(position + size * 0.5).to_array()), "{max:?}");
    }

    let cameras: Vec<_> = capture.cameras.iter().map(|camera| camera.node).collect();
    assert_eq!(cameras, [NodeId::new(3), NodeId::new(4)]);
    let first = &capture.cameras[0];
    assert!(near(
        &first.world,
        &Mat4::from_translation(Vec3::new(0.0, 0.0, 5.0)).to_cols_array()
    ));
    let CameraProjection::Perspective {
        fov_y_degrees,
        near: near_plane,
        far,
    } = first.projection
    else {
        panic!("a perspective camera");
    };
    assert!(near(&[fov_y_degrees, near_plane, far], &[50.0, 0.1, 100.0]));
    assert!(matches!(
        capture.cameras[1].projection,
        CameraProjection::Orthographic { height, .. } if (height - 6.0).abs() < 1e-4
    ));
    assert!(renderer.take_scene_captures().is_empty(), "taking drains");
}

#[test]
fn an_instance_3d_reports_each_copy_of_its_objects() {
    let composition: Composition = scene(
        SIZE,
        json!([
            { "id": 1, "type": "shape_3d", "params": { "size": [1, 1, 1] } },
            { "id": 2, "type": "instance_3d",
              "params": { "count": 3, "position": "=vec3(instance * 2, 0, 0)" },
              "in": { "scene": 1 } },
            { "id": 100, "type": "render_3d", "in": { "scene": 2 } },
        ]),
    )
    .into();
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    renderer.set_scene_capture(true);
    draw(&mut renderer, &composition, 0.0);

    let [capture] = renderer
        .take_scene_captures()
        .try_into()
        .expect("one capture");
    let copies: Vec<_> = capture
        .objects
        .iter()
        .map(|object| (object.node, object.bounds.min[0], object.bounds.max[0]))
        .collect();
    assert_eq!(
        copies,
        [
            (NodeId::new(1), -0.5, 0.5),
            (NodeId::new(1), 1.5, 2.5),
            (NodeId::new(1), 3.5, 4.5)
        ]
    );
}

#[test]
fn identical_copies_and_the_parts_of_a_box_are_each_reported_as_what_they_are() {
    // Three copies placed alike, of a box that draws in a part for each of two materials.
    let composition: Composition = scene(
        SIZE,
        json!([
            { "id": 5, "type": "material_3d", "params": { "color": RED } },
            { "id": 1, "type": "shape_3d", "in": { "top_material": 5 } },
            { "id": 2, "type": "instance_3d", "params": { "count": 3 }, "in": { "scene": 1 } },
            { "id": 100, "type": "render_3d", "in": { "scene": 2 } },
        ]),
    )
    .into();
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    renderer.set_scene_capture(true);
    draw(&mut renderer, &composition, 0.0);

    let [capture] = renderer
        .take_scene_captures()
        .try_into()
        .expect("one capture");
    let nodes: Vec<_> = capture.objects.iter().map(|object| object.node).collect();
    assert_eq!(nodes, [NodeId::new(1); 3]);
}

#[test]
fn a_particles_3d_is_one_object_covering_all_its_particles() {
    let composition: Composition = scene(
        SIZE,
        json!([
            { "id": 1, "type": "shape_3d", "params": { "position": [-1, 0, 0] } },
            { "id": 2, "type": "shape_3d", "params": { "position": [1, 0, 0] } },
            merge(3, &[1, 2], json!({})),
            { "id": 4, "type": "particles_3d", "params": { "count": 4, "speed": 0 },
              "in": { "object": 3 } },
            { "id": 100, "type": "render_3d", "in": { "scene": 4 } },
        ]),
    )
    .frames(30)
    .into();
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    renderer.set_scene_capture(true);
    draw(&mut renderer, &composition, 0.0);

    let [capture] = renderer
        .take_scene_captures()
        .try_into()
        .expect("one capture");
    let [system] = capture.objects.as_slice() else {
        panic!("one object, got {:?}", capture.objects);
    };
    assert_eq!(system.node, NodeId::new(4));
    assert!(system.bounds.min[0] <= -1.5 && system.bounds.max[0] >= 1.5);
}

#[test]
fn a_render_only_a_later_motion_blur_sample_reaches_is_captured_once() {
    // The switch shows the background at the first sample and the render at the second.
    let composition: Composition = scene(
        SIZE,
        json!([
            { "id": 1, "type": "background", "params": { "paint": [200, 210, 220, 255] } },
            { "id": 2, "type": "shape_3d" },
            { "id": 3, "type": "render_3d", "in": { "scene": 2 } },
            { "id": 4, "type": "switch", "params": { "selected_layer": "=if(frame < 0.9, 0, 1)" },
              "in": { "layers": [1, 3] } },
            { "id": 5, "type": "motion_blur", "params": { "samples": 2, "shutter": 180.0 },
              "in": { "source": 4 } },
        ]),
    )
    .frames(4)
    .into();
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    renderer.set_scene_capture(true);
    draw(&mut renderer, &composition, 1.0);

    let renders: Vec<_> = renderer
        .take_scene_captures()
        .iter()
        .map(|capture| capture.render)
        .collect();
    assert_eq!(renders, [NodeId::new(3)]);
}

/// `two_shapes`, drawn through a camera at `eye` looking at the origin, as the scene's own.
fn viewed_from(eye: [f64; 3]) -> Scene {
    two_shapes(&[camera(3, json!({ "position": eye, "look_at": true }))])
}

fn from_the_side() -> SceneCamera {
    SceneCamera {
        world: lumen_3d::look_at(Vec3::new(4.0, 0.0, 3.0), Vec3::ZERO).to_cols_array(),
        projection: CameraProjection::Perspective {
            fov_y_degrees: 50.0,
            near: 0.1,
            far: 100.0,
        },
    }
}

#[test]
fn an_override_camera_draws_what_the_scene_would_through_that_camera_until_cleared() {
    let gpu = gpu();
    let media = TestMedia::default();
    let front: Composition = viewed_from([0.0, 0.0, 5.0]).into();
    let side: Composition = viewed_from([4.0, 0.0, 3.0]).into();
    let expected_front = render(&mut gpu.renderer(), &front, 0.0, &media);
    let expected_side = render(&mut gpu.renderer(), &side, 0.0, &media);
    assert_ne!(expected_front.rgba, expected_side.rgba);

    let mut renderer = gpu.renderer();
    assert_eq!(
        render(&mut renderer, &front, 0.0, &media).rgba,
        expected_front.rgba
    );
    renderer.set_camera_override(Some(from_the_side()));
    assert_eq!(
        render(&mut renderer, &front, 0.0, &media).rgba,
        expected_side.rgba
    );
    renderer.set_camera_override(None);
    assert_eq!(
        render(&mut renderer, &front, 0.0, &media).rgba,
        expected_front.rgba
    );
}

#[test]
fn capture_places_every_frame_again_while_on_and_reports_nothing_while_off() {
    // The merge fades in, so it records again on every frame and reads what is kept of the
    // static render.
    let composition: Composition = scene(
        SIZE,
        json!([
            { "id": 1, "type": "shape_3d" },
            { "id": 7, "type": "render_3d", "in": { "scene": 1 } },
            { "id": 9, "type": "background", "params": { "paint": [200, 210, 220, 255] } },
            { "id": 10, "type": "merge", "params": { "opacity": "=0.5 + frame * 0.25" },
              "in": { "base": 7, "overlay": 9 } },
        ]),
    )
    .frames(4)
    .into();
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    draw(&mut renderer, &composition, 0.0);
    draw(&mut renderer, &composition, 1.0);
    assert_eq!(renderer.scene_walk_count(), 1, "the render is kept");
    assert!(renderer.take_scene_captures().is_empty());

    renderer.set_scene_capture(true);
    draw(&mut renderer, &composition, 1.0);
    draw(&mut renderer, &composition, 1.0);
    assert_eq!(renderer.scene_walk_count(), 3);
    let captures: Vec<SceneCapture> = renderer.take_scene_captures();
    assert_eq!(captures.len(), 2);
    assert_eq!(captures[0], captures[1]);

    renderer.set_scene_capture(false);
    draw(&mut renderer, &composition, 2.0);
    assert_eq!(
        renderer.scene_walk_count(),
        3,
        "the kept render is used again"
    );
    assert!(renderer.take_scene_captures().is_empty());
}

#[test]
fn colour_and_depth_of_one_render_are_one_capture_a_frame() {
    let nodes = vec![
        json!({ "id": 1, "type": "shape_3d" }),
        json!({ "id": 7, "type": "render_3d", "params": { "depth_near": 3, "depth_far": 9 },
               "in": { "scene": 1 } }),
        json!({ "id": 9, "type": "background", "params": { "paint": [200, 210, 220, 255] } }),
        json!({ "id": 10, "type": "merge", "params": { "mask_mode": "luma" },
               "in": { "overlay": 9 } }),
    ];
    let mut composition = scene(SIZE, json!(nodes)).frames(2);
    composition.0["connections"]
        .as_array_mut()
        .expect("connections")
        .extend([
            json!({ "from_node": 7, "from_port": "color", "to_node": 10, "to_port": "base" }),
            json!({ "from_node": 7, "from_port": "depth", "to_node": 10, "to_port": "mask" }),
        ]);
    let composition: Composition = composition.into();
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    renderer.set_scene_capture(true);
    for frame in 0..2 {
        draw(&mut renderer, &composition, f64::from(frame));
    }
    let renders: Vec<_> = renderer
        .take_scene_captures()
        .iter()
        .map(|capture| capture.render)
        .collect();
    assert_eq!(renders, [NodeId::new(7), NodeId::new(7)]);
}
