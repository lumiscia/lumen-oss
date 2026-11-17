//! `extrude_3d` and `lathe_3d`: 2D outlines turned into solids drawn by a `render_3d`.

use lumen_engine::{composition::Composition, gpu::RenderTarget};
use serde_json::{Value, json};

use crate::support::scene::{SIZE, material, merge, ortho, render_3d_with};
use crate::support::{Frame, GREEN, Scene, TestMedia, gpu, render};

/// Frame pixels to a world unit under the orthographic camera `ortho` makes, which sees 4 units
/// across the canvas.
const PER_UNIT: f32 = SIZE.0 as f32 / 4.0;

fn shape_2d(params: Value) -> Value {
    json!({ "id": 1, "type": "shape_2d", "params": params })
}

/// A solid of node kind `kind` with `params`, from the geometry node `geometry` (id 1), unlit
/// green, seen by `camera` (id 4). Nodes in `extra` come before the solid.
fn solid_scene(kind: &str, geometry: Value, params: Value, camera: Value) -> Scene {
    render_3d_with(
        json!({}),
        vec![
            geometry,
            material(3, json!({ "color": GREEN, "unlit": true })),
            json!({ "id": 2, "type": kind, "params": params,
                "in": { "geometry": 1, "material": 3 } }),
            camera,
            merge(5, &[2, 4], json!({})),
        ],
    )
}

fn extrude(geometry: Value, params: Value) -> Scene {
    solid_scene("extrude_3d", geometry, params, ortho(4, json!({})))
}

/// A camera looking straight down at the origin from above, 6 units across.
fn from_above() -> Value {
    ortho(
        4,
        json!({ "position": [0, 4, 0], "rotation": [-90, 0, 0], "ortho_height": 6 }),
    )
}

fn frame(scene: Scene) -> Frame {
    render(
        &mut gpu().renderer(),
        &scene.into(),
        0.0,
        &TestMedia::default(),
    )
}

fn covered(frame: &Frame, x: u32, y: u32) -> bool {
    frame.pixel(x, y)[3] == 255
}

/// The pixel of the frame at world `(x, y)` under the camera `ortho` makes.
fn at(x: f32, y: f32) -> (u32, u32) {
    let c = SIZE.0 as f32 / 2.0;
    ((c + x * PER_UNIT) as u32, (c - y * PER_UNIT) as u32)
}

#[test]
fn an_extruded_shape_covers_its_footprint_with_y_flipped_about_pixel_zero() {
    // 200 by 100 pixels centred on the origin, so 2 by 1 units.
    let centred = frame(extrude(
        shape_2d(json!({ "width": 200, "height": 100, "position": [-100, -50] })),
        json!({}),
    ));
    for y in 0..SIZE.1 {
        for x in 0..SIZE.0 {
            let inside = (18..46).contains(&x) && (26..38).contains(&y);
            let outside = !(14..50).contains(&x) || !(22..42).contains(&y);
            if inside {
                assert!(
                    covered(&centred, x, y),
                    "({x}, {y}) is inside the footprint"
                );
            } else if outside {
                assert_eq!(
                    centred.pixel(x, y)[3],
                    0,
                    "({x}, {y}) is outside the footprint"
                );
            }
        }
    }

    // At position (0, 0) the shape lies right of the origin, and below it: 2D Y points down.
    let corner = frame(extrude(
        shape_2d(json!({ "width": 100, "height": 100 })),
        json!({}),
    ));
    let (x, y) = at(0.5, -0.5);
    assert!(covered(&corner, x, y));
    for (x, y) in [at(-0.5, -0.5), at(0.5, 0.5), at(-0.5, 0.5)] {
        assert!(!covered(&corner, x, y), "({x}, {y}) is off the square");
    }

    // Each pixel is `units_per_pixel` across, and an ellipse is round.
    let ellipse = frame(extrude(
        shape_2d(json!({
            "geometry_kind": "ellipse", "width": 100, "height": 100, "position": [-50, -50],
        })),
        json!({ "units_per_pixel": 0.02 }),
    ));
    for (x, y) in [at(0.0, 0.0), at(0.9, 0.0), at(0.0, -0.9), at(0.6, 0.6)] {
        assert!(covered(&ellipse, x, y), "({x}, {y}) is in the circle");
    }
    for (x, y) in [at(1.1, 0.0), at(0.9, 0.9), at(-0.9, -0.9)] {
        assert!(!covered(&ellipse, x, y), "({x}, {y}) is outside the circle");
    }
}

#[test]
fn an_extruded_path_keeps_its_holes_and_rotates_with_its_own_transform() {
    // A square with a square hole wound against it, which both fill rules leave open.
    let data =
        "M -100 -100 L 100 -100 L 100 100 L -100 100 Z M -40 -40 L -40 40 L 40 40 L 40 -40 Z";
    let holed = frame(extrude(
        json!({ "id": 1, "type": "path_2d", "params": { "data": data } }),
        json!({}),
    ));
    assert!(
        covered(&holed, 32, 32 - 11),
        "the ring around the hole is solid"
    );
    assert!(!covered(&holed, 32, 32), "the hole is open");

    // Turned a quarter about its middle, a wide bar becomes a tall one.
    let bar = |rotation: f64| {
        frame(extrude(
            shape_2d(json!({
                "width": 200, "height": 40, "position": [-100, -20], "anchor": [100, 20],
                "rotation": rotation,
            })),
            json!({}),
        ))
    };
    let (flat, turned) = (bar(0.0), bar(90.0));
    assert!(covered(&flat, 32 + 12, 32) && !covered(&flat, 32, 32 + 12));
    assert!(covered(&turned, 32, 32 + 12) && !covered(&turned, 32 + 12, 32));
}

#[test]
fn depth_shows_as_side_walls_when_the_solid_is_turned() {
    let area = |depth: f64| {
        let turned = extrude(
            shape_2d(json!({ "width": 200, "height": 100, "position": [-100, -50] })),
            json!({ "depth": depth, "rotation": [70, 0, 0] }),
        );
        let frame = frame(turned);
        (0..SIZE.1)
            .flat_map(|y| (0..SIZE.0).map(move |x| (x, y)))
            .filter(|&(x, y)| covered(&frame, x, y))
            .count()
    };
    let (flat, deep) = (area(0.0), area(1.0));
    assert!(deep > flat * 2, "{deep} pixels deep against {flat} flat");
}

#[test]
fn a_lathed_rectangle_is_a_ring_seen_from_above() {
    // 1 to 2 units from the axis, and 0 to 1 high.
    let profile = shape_2d(json!({ "width": 100, "height": 100, "position": [100, -100] }));
    let ring = frame(solid_scene(
        "lathe_3d",
        profile.clone(),
        json!({}),
        from_above(),
    ));
    let per_unit = SIZE.0 as f32 / 6.0;
    let pixel = |x: f32, z: f32| {
        let c = SIZE.0 as f32 / 2.0;
        ((c + x * per_unit) as u32, (c + z * per_unit) as u32)
    };
    let (x, y) = pixel(0.0, 0.0);
    assert!(!covered(&ring, x, y), "the middle is empty");
    for (x, z) in [(1.5, 0.0), (-1.5, 0.0), (0.0, 1.5), (0.0, -1.5), (1.1, 1.1)] {
        let (px, py) = pixel(x, z);
        assert!(covered(&ring, px, py), "({x}, {z}) is on the ring");
    }
    for (x, z) in [(0.6, 0.0), (2.4, 0.0), (1.8, 1.8)] {
        let (px, py) = pixel(x, z);
        assert!(!covered(&ring, px, py), "({x}, {z}) is off the ring");
    }

    // Half a turn from +X towards -Z leaves a wedge on one side.
    let half = frame(solid_scene(
        "lathe_3d",
        profile,
        json!({ "sweep": 180 }),
        from_above(),
    ));
    let (x, y) = pixel(0.0, -1.5);
    assert!(covered(&half, x, y), "the -Z side is spun");
    let (x, y) = pixel(0.0, 1.5);
    assert!(!covered(&half, x, y), "the +Z side is left open");
}

fn failure(scene: Scene) -> String {
    gpu()
        .renderer()
        .render(
            &Composition::from(scene),
            0.0,
            &TestMedia::default(),
            RenderTarget::Owned,
        )
        .expect_err("the render fails")
        .to_string()
}

#[test]
fn an_offset_2d_in_the_geometry_names_the_solid() {
    for (kind, verb) in [("extrude_3d", "extrude"), ("lathe_3d", "spin")] {
        let mut scene = solid_scene(
            kind,
            shape_2d(json!({ "width": 100, "height": 100 })),
            json!({}),
            ortho(4, json!({})),
        );
        let nodes = scene.0["nodes"].as_array_mut().expect("nodes");
        nodes.push(json!({ "id": 6, "type": "offset_2d", "params": { "offset": 0 } }));
        let connections = scene.0["connections"].as_array_mut().expect("connections");
        connections.retain(|c| c["to_node"] != 2 || c["to_port"] != "geometry");
        connections.push(json!({ "from_node": 1, "to_node": 6, "to_port": "geometry" }));
        connections.push(json!({ "from_node": 6, "to_node": 2, "to_port": "geometry" }));
        let error = failure(scene);
        assert!(
            error.contains(&format!(
                "{kind} can't {verb} an offset_2d outline yet; remove the offset_2d"
            )) && error.contains("node 2"),
            "{error}"
        );
    }
}

#[test]
fn a_solid_past_its_limits_fails_naming_the_node() {
    // 512 steps around a profile of 600 corners holds more than 262144 vertices.
    let data: String = (0..600)
        .map(|i| format!("{} {} {}", if i == 0 { "M" } else { "L" }, 100 + i % 7, i))
        .collect::<Vec<_>>()
        .join(" ");
    let error = failure(solid_scene(
        "lathe_3d",
        json!({ "id": 1, "type": "path_2d", "params": { "data": data } }),
        json!({ "segments": 512 }),
        from_above(),
    ));
    assert!(
        error.contains("node 2") && error.contains("more than 262144 vertices"),
        "{error}"
    );

    let error = failure(extrude(
        json!({ "id": 1, "type": "path_2d", "params": { "data": "M 0 0 L 1 0 L" } }),
        json!({}),
    ));
    assert!(
        error.contains("node 2") && error.contains("path_2d node 1: `data` is malformed"),
        "{error}"
    );
}

#[test]
fn a_static_outline_is_built_once_and_a_changing_one_is_built_again() {
    let gpu = gpu();
    let media = TestMedia::default();
    let held = |params: Value| {
        let composition: Composition =
            extrude(shape_2d(json!({ "width": 100, "height": 100 })), params)
                .frames(3)
                .into();
        let mut renderer = gpu.renderer();
        (0..3)
            .map(|frame| {
                render(&mut renderer, &composition, f64::from(frame), &media);
                renderer.cache_stats().text_meshes
            })
            .collect::<Vec<u64>>()
    };
    // Turning the solid draws it again each frame without building it again.
    let turning = held(json!({ "rotation": "=vec3(0, frame * 10, 0)" }));
    assert!(turning[0] > 0, "the mesh is kept: {turning:?}");
    assert_eq!(turning[0], turning[2], "built once: {turning:?}");
    // A plain extrusion is scaled to its depth, so changing it builds nothing again.
    let deepening = held(json!({ "depth": "=0.25 + frame * 0.1" }));
    assert_eq!(deepening[0], deepening[2], "built once: {deepening:?}");
    // A bevel is sized against the depth, so each depth builds a mesh, which the cache keeps.
    let bevelled = held(json!({ "depth": "=0.25 + frame * 0.1", "bevel_size": 0.05 }));
    assert!(bevelled[2] > bevelled[0], "built again: {bevelled:?}");
}
