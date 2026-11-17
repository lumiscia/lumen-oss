//! The shadows that `render_3d` lights cast and shadow catchers show.

use lumen_engine::composition::Composition;
use serde_json::{Value, json};

use crate::support::scene::{
    GroundView, SIZE, every_pixel, flat, fox, fox_media, frame_of, half_blue, image, light,
    material, merge, model, plane, plate, quads, render_3d_with, with_material,
};
use crate::support::{
    CLEAR, Case, Expect, Frame, RED, Scene, TestMedia, check, gpu, render, scene,
};

/// `params` with shadows cast or not.
fn casting(params: &Value, cast: bool) -> Value {
    let mut params = params.clone();
    params["cast_shadows"] = json!(cast);
    params
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
        let mut catcher_material = json!({ "shadow_catcher": true });
        for (name, value) in catcher.as_object().expect("catcher") {
            catcher_material[name] = value.clone();
        }
        let objects = vec![
            json!({ "id": 1, "type": "shape_3d", "params": { "size": [1, 0.2, 1], "position": [0, 1, 0] } }),
            material(3, catcher_material),
            with_material(
                json!({ "id": 2, "type": "shape_3d", "params": {
                    "shape": "plane", "size": [6, 1, 6], "position": [0, 0.01, 0] } }),
                3,
            ),
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
    // Half the backdrop's light shows through half-transparent black.
    let [r, g, b, _] = BACKDROP;
    let darkened = crate::color::opaque([r, g, b].map(|code| 0.5 * crate::color::linear(code)));
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

/// A slab a unit over the middle of an unlit plane of [`COLOR`] with `material_params`, lit from
/// straight above and seen from above, so its shadow falls on the middle of the plane.
fn unlit_plane_under_a_slab(material_params: Value) -> Scene {
    let mut params = json!({ "color": COLOR, "unlit": true });
    for (name, value) in material_params.as_object().expect("params") {
        params[name] = value.clone();
    }
    GroundView::WHOLE.scene(
        vec![
            json!({ "id": 1, "type": "shape_3d", "params": { "size": [1, 0.2, 1], "position": [0, 1, 0] } }),
            material(3, params),
            with_material(
                json!({ "id": 2, "type": "shape_3d", "params": {
                    "shape": "plane", "size": [6, 1, 6], "position": [0, 0.01, 0] } }),
                3,
            ),
            light(
                60,
                json!({ "light": "directional", "intensity": 1, "rotation": [-90, 0, 0],
                        "cast_shadows": true }),
            ),
        ],
        json!({}),
    )
}

const COLOR: &str = "#e0c080";
const COLOR_CODES: [u8; 3] = [0xe0, 0xc0, 0x80];

#[test]
fn an_unlit_surface_darkens_in_shadow_by_its_unlit_shadows() {
    let view = GroundView::WHOLE;
    let (shadow_x, shadow_y) = view.pixel(0.0, 0.0);
    let (lit_x, lit_y) = view.pixel(2.5, 2.5);
    let [r, g, b] = COLOR_CODES;
    let base = [r, g, b, 255];
    let halved = crate::color::opaque(COLOR_CODES.map(|code| 0.5 * crate::color::linear(code)));
    let probes =
        |shadow: [u8; 4]| Expect::Probes(vec![(shadow_x, shadow_y, shadow), (lit_x, lit_y, base)]);
    check(vec![
        Case::new(
            "half dark in the shadow and exact outside it",
            unlit_plane_under_a_slab(json!({ "unlit_shadows": 0.5 })),
            probes(halved),
        )
        .tolerance(1),
        Case::new(
            "black in a full shadow at 1",
            unlit_plane_under_a_slab(json!({ "unlit_shadows": 1 })),
            probes([0, 0, 0, 255]),
        )
        .tolerance(1),
        Case::new(
            "unchanged by default",
            unlit_plane_under_a_slab(json!({})),
            probes(base),
        )
        .tolerance(1),
        Case::new(
            "unchanged where it doesn't receive shadows",
            unlit_plane_under_a_slab(json!({ "unlit_shadows": 0.5, "receive_shadows": false })),
            probes(base),
        )
        .tolerance(1),
    ]);
}

#[test]
fn shadow_catchers_hide_what_is_behind_them() {
    let view = GroundView::WHOLE;
    // A red box sunk below a catcher shows nowhere.
    let mut objects = flat(
        1,
        3,
        json!({ "position": [0, -0.6, 0] }),
        json!({ "color": RED }),
    )
    .to_vec();
    objects.extend([
        material(4, json!({ "shadow_catcher": true })),
        with_material(
            json!({ "id": 2, "type": "shape_3d", "params": {
                "shape": "plane", "size": [6, 1, 6], "position": [0, 0.01, 0] } }),
            4,
        ),
    ]);
    let scene = view.scene(objects, json!({}));
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
            objects.extend([
                material(
                    3,
                    json!({ "unlit": true, "double_sided": true, "shadow_catcher": true }),
                ),
                with_material(
                    plane(
                        2,
                        10,
                        json!({ "width": 6, "rotation": [-90, 0, 0], "position": [0, 0.01, 0] }),
                    ),
                    3,
                ),
            ]);
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
                    material(7, json!({ "roughness": 1, "cast_shadows": false })),
                    with_material(
                        json!({ "id": 1, "type": "shape_3d", "params": {
                            "shape": "plane", "size": [2.0 * half, 1, 2.0 * half] } }),
                        7,
                    ),
                    with_material(
                        json!({ "id": 6, "type": "shape_3d", "params": {
                            "shape": "plane", "size": [6, 1, 6], "position": [0, 0.02, 0] } }),
                        7,
                    ),
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
            // The ground's material, which `GroundView` lists first.
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
            material(6, json!({ "roughness": 1, "cast_shadows": false })),
            material(7, json!({ "receive_shadows": false })),
            with_material(
                json!({ "id": 1, "type": "shape_3d", "params": {
                    "shape": "plane", "size": [10000, 1, 10000] } }),
                6,
            ),
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
            nodes.push(with_material(
                json!({ "id": id, "type": "shape_3d", "params": {
                    "size": [2, 1, 2], "position": [x, 0.5, z] } }),
                7,
            ));
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
