//! `material_3d`'s and `model_3d`'s `projector`: a texture projected onto the scene from a
//! camera, in place of texture coordinates.

use lumen_3d::glam::Vec3;
use lumen_3d::{Camera, Projection};
use lumen_engine::{composition::Composition, gpu::RenderTarget};
use serde_json::{Value, json};

use crate::support::scene::{
    CENTER, Quad, frame_of, gradient, image, material, merge, model, model_glb, plate, render_3d,
    unlit,
};
use crate::support::{BLUE, Frame, GREEN, RED, TestMedia, gpu};

const WHITE: [u8; 4] = [255; 4];
/// The `camera_3d` materials project from, which isn't in the scene drawn.
const PROJECTOR: u32 = 20;

/// A `camera_3d` at `position` aimed at `target`, with a vertical view `fov` degrees across.
fn camera(id: u32, position: [f32; 3], target: [f32; 3], fov: f32) -> Value {
    json!({ "id": id, "type": "camera_3d", "params": {
        "position": position, "look_at": true, "target": target, "fov": fov } })
}

/// The camera [`camera`] makes.
fn camera_of(position: [f32; 3], target: [f32; 3], fov: f32) -> Camera {
    Camera {
        world: lumen_3d::look_at(Vec3::from(position), Vec3::from(target)),
        projection: Projection::Perspective {
            fov_y: fov.to_radians(),
        },
        near: 0.05,
        far: 1000.0,
    }
}

/// The pixel of the square canvas that `camera` shows `point` in.
fn pixel_of(camera: &Camera, point: Vec3) -> (u32, u32) {
    let ndc = (camera.projection_matrix(1.0) * camera.view()).project_point3(point);
    let half = CENTER as f32;
    let (x, y) = ((ndc.x + 1.0) * half, (1.0 - ndc.y) * half);
    assert!(
        (0.0..2.0 * half).contains(&x) && (0.0..2.0 * half).contains(&y),
        "{point} is in view"
    );
    (x as u32, y as u32)
}

/// An unlit white `material_3d` projecting the image `plate`, node 10, from [`PROJECTOR`].
fn projected(id: u32, params: Value) -> Value {
    let mut material = material(id, params);
    material["params"]["unlit"] = json!(true);
    material["in"] = json!({ "texture": 10, "projector": PROJECTOR });
    material
}

fn shape(id: u32, params: Value, material: u32) -> Value {
    json!({ "id": id, "type": "shape_3d", "params": params, "in": { "material": material } })
}

/// A plane facing +Z, `size` units across, at `position`.
fn wall(id: u32, size: f32, position: [f32; 3], material: u32) -> Value {
    shape(
        id,
        json!({ "shape": "plane", "size": [size, 1, size], "rotation": [90, 0, 0],
            "position": position }),
        material,
    )
}

#[test]
fn a_plane_facing_the_projector_seen_through_it_shows_the_texture() {
    let nodes = vec![
        plate(10),
        projected(11, json!({})),
        wall(1, 6.0, [0.0, 0.0, 0.0], 11),
        camera(PROJECTOR, [0.0, 0.0, 5.0], [0.0; 3], 40.0),
        camera(21, [0.0, 0.0, 5.0], [0.0; 3], 40.0),
        merge(3, &[1, 21], json!({})),
    ];
    let frame = frame_of(render_3d(nodes), &gradient());
    for (x, y) in (0..64).flat_map(|y| (0..64).map(move |x| (x, y))) {
        let expected = [x as u8 * 4, y as u8 * 4, 128, 255];
        let pixel = frame.pixel(x, y);
        assert!(
            pixel.iter().zip(expected).all(|(&a, b)| a.abs_diff(b) <= 2),
            "({x}, {y}) shows texel ({x}, {y}): got {pixel:?}, wanted {expected:?}"
        );
    }
}

#[test]
fn a_projected_point_keeps_its_texel_wherever_the_scene_is_seen_from() {
    // A red square of 8 texels centred on texel (42, 18) of a white image.
    let media = image(|x, y| {
        if (38..46).contains(&x) && (14..22).contains(&y) {
            RED
        } else {
            WHITE
        }
    });
    // Texel (42, 18) is at (0.3125, 0.4375) of the projector's view, on the box's front 4.5 units
    // away, where its view is 2 · 4.5 · tan 20° across.
    let reach = 4.5 * 20f32.to_radians().tan();
    let marker = Vec3::new(0.3125 * reach, 0.4375 * reach, 0.5);
    for position in [[0.0, 0.0, 5.0], [2.0, 1.0, 6.0], [-2.5, -1.5, 5.0]] {
        let nodes = vec![
            plate(10),
            projected(11, json!({})),
            shape(1, json!({ "size": [3, 3, 1] }), 11),
            camera(PROJECTOR, [0.0, 0.0, 5.0], [0.0; 3], 40.0),
            camera(21, position, [0.0; 3], 40.0),
            merge(3, &[1, 21], json!({})),
        ];
        let frame = frame_of(render_3d(nodes), &media);
        let seen = camera_of(position, [0.0; 3], 40.0);
        let (x, y) = pixel_of(&seen, marker);
        assert_eq!(frame.pixel(x, y), RED, "the marker, seen from {position:?}");
        let (x, y) = pixel_of(&seen, marker - Vec3::new(0.5, 0.5, 0.0));
        assert_eq!(
            frame.pixel(x, y),
            WHITE,
            "beside the marker, seen from {position:?}"
        );
    }
}

#[test]
fn faces_turned_away_from_the_projector_or_out_of_its_view_show_the_colour_alone() {
    // A projector on +X with a 30° view sees all of the box's +X face, from 4.5 units away, but not
    // the second box, 3 units along +Z.
    let nodes = vec![
        plate(10),
        projected(11, json!({})),
        shape(1, json!({}), 11),
        shape(2, json!({ "position": [0, 0, 3] }), 11),
        camera(PROJECTOR, [5.0, 0.0, 0.0], [0.0; 3], 30.0),
        camera(21, [5.0, 2.0, 5.0], [0.0, 0.0, 1.5], 50.0),
        merge(3, &[1, 2, 21], json!({})),
    ];
    let frame = frame_of(render_3d(nodes), &image(|_, _| BLUE));
    let seen = camera_of([5.0, 2.0, 5.0], [0.0, 0.0, 1.5], 50.0);
    let at = |point: [f32; 3]| {
        let (x, y) = pixel_of(&seen, Vec3::from(point));
        frame.pixel(x, y)
    };
    assert_eq!(at([0.5, 0.0, 0.0]), BLUE, "the face towards the projector");
    assert_eq!(
        at([0.25, 0.0, 0.5]),
        WHITE,
        "the front face, in view but turned away"
    );
    assert_eq!(
        at([0.5, 0.0, 3.0]),
        WHITE,
        "the second box's face towards the projector, out of its view"
    );
}

/// A wall 2 units behind a red box, which hides part of it from the projector, seen from the side.
fn occluded(occlusion: bool) -> Frame {
    let nodes = vec![
        plate(10),
        projected(11, json!({ "projector_occlusion": occlusion })),
        material(12, json!({ "color": RED, "unlit": true })),
        wall(1, 6.0, [0.0, 0.0, -1.0], 11),
        shape(
            2,
            json!({ "size": [0.6, 0.6, 0.6], "position": [0, 0, 1] }),
            12,
        ),
        camera(PROJECTOR, [0.0, 0.0, 5.0], [0.0; 3], 40.0),
        camera(21, [3.0, 0.0, 5.0], [0.0, 0.0, -1.0], 40.0),
        merge(3, &[1, 2, 21], json!({})),
    ];
    frame_of(render_3d(nodes), &image(|_, _| BLUE))
}

#[test]
fn occlusion_hides_the_projection_where_the_projector_cant_see() {
    let seen = camera_of([3.0, 0.0, 5.0], [0.0, 0.0, -1.0], 40.0);
    // The box's shadow from the projector covers the wall's middle, out to about 0.5 units.
    let (hidden_x, hidden_y) = pixel_of(&seen, Vec3::new(0.0, 0.0, -1.0));
    let (open_x, open_y) = pixel_of(&seen, Vec3::new(1.2, 0.0, -1.0));
    let (box_x, box_y) = pixel_of(&seen, Vec3::new(0.3, 0.0, 1.0));
    let on = occluded(true);
    assert_eq!(on.pixel(hidden_x, hidden_y), WHITE, "behind the box");
    assert_eq!(on.pixel(open_x, open_y), BLUE, "clear of the box");
    assert_eq!(on.pixel(box_x, box_y), RED, "the box keeps its own colour");
    let off = occluded(false);
    assert_eq!(off.pixel(hidden_x, hidden_y), BLUE, "behind the box");
    assert_eq!(off.pixel(open_x, open_y), BLUE, "clear of the box");
}

#[test]
fn instance_3d_copies_show_the_part_of_the_projection_where_they_stand() {
    // The projector sees from -1.82 to 1.82 across the origin: red left of 0 and green right.
    let halves = image(|x, _| if x < CENTER { RED } else { GREEN });
    let nodes = vec![
        plate(10),
        projected(11, json!({})),
        wall(1, 1.5, [0.0; 3], 11),
        camera(PROJECTOR, [0.0, 0.0, 5.0], [0.0; 3], 40.0),
        json!({ "id": 2, "type": "instance_3d", "in": { "scene": 1 },
            "params": { "count": 2, "position": "=vec3(instance * 2 - 1, 0, 0)" } }),
    ];
    let frame = frame_of(render_3d(nodes), &halves);
    // Half a unit either side of the origin, about 9 pixels, is the inner part of each copy,
    // whose own texture coordinates would show the other half there.
    assert_eq!(frame.pixel(CENTER - 9, CENTER), RED, "the left copy");
    assert_eq!(frame.pixel(CENTER + 9, CENTER), GREEN, "the right copy");
}

#[test]
fn a_model_3d_projects_its_texture_onto_the_material_it_names() {
    let halves = image(|x, _| if x < CENTER { RED } else { GREEN });
    // A quad whose texture coordinates run right to left, so they would show green on the left.
    let screen = model_glb(
        json!([unlit("screen", [1.0; 4])]),
        None,
        &[Quad {
            edges: [-1.5, -1.0, 1.5, 1.0],
            material: 0,
            normal: [0.0, 0.0, 1.0],
            mirrored_u: true,
        }],
    );
    let mut screen_model = model(
        1,
        json!({ "source": "screen.glb", "texture_material": "screen" }),
    );
    screen_model["in"] = json!({ "texture": 10, "projector": PROJECTOR });
    let nodes = vec![
        plate(10),
        screen_model,
        camera(PROJECTOR, [0.0, 0.0, 5.0], [0.0; 3], 40.0),
        merge(3, &[1], json!({})),
    ];
    let frame = frame_of(render_3d(nodes), &halves.model("screen.glb", screen));
    assert_eq!(frame.pixel(CENTER - 9, CENTER), RED, "left of the middle");
    assert_eq!(
        frame.pixel(CENTER + 9, CENTER),
        GREEN,
        "right of the middle"
    );
}

#[test]
fn a_projector_without_a_texture_fails_naming_the_material() {
    let mut material = projected(11, json!({}));
    material["in"] = json!({ "projector": PROJECTOR });
    let nodes = vec![
        material,
        shape(1, json!({}), 11),
        camera(PROJECTOR, [0.0, 0.0, 5.0], [0.0; 3], 40.0),
        merge(3, &[1], json!({})),
    ];
    let composition = Composition::from(render_3d(nodes));
    let error = gpu()
        .renderer()
        .render(
            &composition,
            0.0,
            &TestMedia::default(),
            RenderTarget::Owned,
        )
        .expect_err("the render fails")
        .to_string();
    assert!(
        error.contains("material_3d node 11")
            && error.contains("a `projector` needs a `texture` to project"),
        "{error}"
    );
}
