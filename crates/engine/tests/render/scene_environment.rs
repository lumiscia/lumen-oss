//! Environment maps: the light they give a surface, what a mirror shows of them and their background.

use exr::prelude::WritableImage;
use lumen_3d::glam::Vec3;
use lumen_engine::{composition::Composition, gpu::RenderTarget};
use serde_json::{Value, json};

use crate::support::scene::{
    CENTER, GroundView, environment_file, every_pixel, frame_of, light, lit_by, render_3d_with,
    sphere,
};
use crate::support::{CLEAR, Case, Expect, Frame, TestMedia, check, gpu, render};

/// Linear `value` as an 8-bit sRGB channel.
fn srgb8(value: f32) -> u8 {
    let encoded = if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round() as u8
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
            .render(
                &render_3d_with(lit_by("map.hdr", json!({})), vec![sphere(json!({}))]).into(),
                0.0,
                &media,
                RenderTarget::Owned,
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
