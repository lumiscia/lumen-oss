//! Rasters shown smaller than they are: when they are filtered and when their mips are made.

use lumen_engine::composition::Composition;
use serde_json::{Value, json};

use crate::support::scene::{merge, ortho, plane, plate, quads, render_3d_with, unlit};
use crate::support::{Frame, TestMedia, gpu, render, scene};

/// A `size` square of single-texel black and white squares: the finest detail a raster holds,
/// whose average is mid grey.
fn checker(size: u32) -> Vec<u8> {
    (0..size * size)
        .flat_map(|index| {
            let (x, y) = (index % size, index / size);
            let value = if (x + y) % 2 == 0 { 255 } else { 0 };
            [value, value, value, 255]
        })
        .collect()
}

/// The pixels of `frame` at least 3 pixels from any that is not opaque.
fn interior(frame: &Frame) -> Vec<(u32, u32)> {
    let opaque = |x: i64, y: i64| {
        (0..frame.width as i64).contains(&x)
            && (0..frame.height as i64).contains(&y)
            && frame.pixel(x as u32, y as u32)[3] == 255
    };
    let mut pixels = Vec::new();
    for y in 0..frame.height {
        for x in 0..frame.width {
            let near = (-3..=3).all(|dy| (-3..=3).all(|dx| opaque(x as i64 + dx, y as i64 + dy)));
            if near {
                pixels.push((x, y));
            }
        }
    }
    pixels
}

/// A plane tilted away from a camera, showing the `size` square `pixels` far smaller than it is.
fn tilted_plane(
    pixels: Vec<u8>,
    size: u32,
    supersample: u32,
    sampling: &str,
) -> (Composition, TestMedia) {
    let scene = scene(
        (128, 128),
        json!([
            { "id": 10, "type": "media_in", "params": { "kind": "image", "source": "plate" } },
            plane(1, 10, json!({ "width": 4, "rotation": [-60, 0, 0], "sampling": sampling })),
            { "id": 100, "type": "render_3d", "params": { "supersample": supersample },
              "in": { "scene": 1 } },
        ]),
    );
    (
        scene.into(),
        TestMedia::default().image("plate", size, size, pixels),
    )
}

#[test]
fn a_raster_shown_much_smaller_than_it_is_filtered_instead_of_aliased() {
    let gpu = gpu();
    let render_plane = |pixels: Vec<u8>, supersample: u32| {
        let (composition, media) = tilted_plane(pixels, 512, supersample, "linear");
        render(&mut gpu.renderer(), &composition, 0.0, &media)
    };
    // The average of single-texel squares is the grey of a plane showing nothing finer: what any
    // number of samples per pixel would converge on. Light averages, so half black and half white
    // is the 188 that is half the light of white, not the 128 that is half its code.
    let reference = render_plane([188, 188, 188, 255].repeat(512 * 512), 1);
    let pixels = interior(&reference);
    assert!(
        pixels.len() > 1000,
        "the plane covers {} pixels",
        pixels.len()
    );
    let error = |frame: &Frame| {
        let sum: u32 = pixels
            .iter()
            .map(|&(x, y)| u32::from(frame.pixel(x, y)[0].abs_diff(reference.pixel(x, y)[0])))
            .sum();
        f64::from(sum) / pixels.len() as f64
    };
    // Supersampling averages the colours as stored, so where its finer samples reach into the
    // raster's first level the checker's squares average to the 128 that is half their code,
    // and the grey darkens a little in the rows nearest the camera.
    for (supersample, allowed) in [(1, 5.0), (2, 30.0)] {
        let frame = render_plane(checker(512), supersample);
        let aliasing = error(&frame);
        assert!(
            aliasing < allowed,
            "{supersample}x supersampled: {aliasing} levels from the grey on average"
        );
    }
}

#[test]
fn a_nearest_plane_shows_only_whole_texels_magnified_or_minified() {
    let gpu = gpu();
    // How many pixels inside the plane blend the checker's black and white.
    let blended = |(composition, media): (Composition, TestMedia)| {
        let frame = render(&mut gpu.renderer(), &composition, 0.0, &media);
        let pixels = interior(&frame);
        assert!(
            pixels.len() > 1000,
            "the plane covers {} pixels",
            pixels.len()
        );
        pixels
            .iter()
            .filter(|&&(x, y)| (2..254).contains(&frame.pixel(x, y)[0]))
            .count()
    };
    // Eight texels across 64 pixels.
    let magnified = render_3d_with(
        json!({ "supersample": 1 }),
        vec![
            plate(10),
            plane(1, 10, json!({ "width": 4, "sampling": "nearest" })),
            ortho(2, json!({})),
            merge(3, &[1, 2], json!({})),
        ],
    );
    let media = TestMedia::default().image("plate", 8, 8, checker(8));
    assert_eq!(blended((magnified.into(), media)), 0);
    assert_eq!(blended(tilted_plane(checker(512), 512, 1, "nearest")), 0);
}

/// A `size` square raster on a model's long thin `screen` rectangle, rolled to run corner to
/// corner of a 160 pixel frame, with `supersample` 1.
fn model_screen(pixels: Vec<u8>, size: u32) -> (Composition, TestMedia) {
    let tv = quads(
        json!([unlit("screen", [0.0, 1.0, 0.0, 1.0])]),
        &[([-2.0, -0.35, 2.0, 0.35], 0)],
    );
    let nodes = json!([
        plate(10),
        {
            "id": 1, "type": "model_3d", "in": { "texture": 10 },
            "params": { "source": "tv.glb", "texture_material": "screen", "rotation": [0, 0, 45] },
        },
        ortho(2, json!({})),
        merge(3, &[1, 2], json!({})),
        { "id": 100, "type": "render_3d", "in": { "scene": 3 },
          "params": { "supersample": 1, "width": 160, "height": 160 } },
    ]);
    (
        scene((160, 160), nodes).into(),
        TestMedia::default()
            .image("plate", size, size, pixels)
            .model("tv.glb", tv),
    )
}

#[test]
fn a_models_raster_is_filtered_where_a_thin_diagonal_surface_squeezes_it() {
    let gpu = gpu();
    // The rectangle's bounds are 133 pixels across each way, more than the raster's 96 texels,
    // but its 96 texels cover only 28 pixels across its width.
    let render_screen = |pixels: Vec<u8>| {
        let (composition, media) = model_screen(pixels, 96);
        render(&mut gpu.renderer(), &composition, 0.0, &media)
    };
    let frame = render_screen(checker(96));
    let pixels = interior(&frame);
    assert!(
        pixels.len() > 500,
        "the surface covers {} pixels",
        pixels.len()
    );
    // Aliased, the checker's squares come and go from pixel to pixel; filtered, they stay as
    // even a grey as the filter's taps make them.
    let values: Vec<f64> = pixels
        .iter()
        .map(|&(x, y)| f64::from(frame.pixel(x, y)[0]))
        .collect();
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let spread = values.iter().map(|v| (v - mean).abs()).sum::<f64>() / values.len() as f64;
    assert!(spread < 12.0, "pixels are {spread} levels from their mean");
}

#[test]
fn a_raster_shown_at_its_own_size_is_not_filtered() {
    let gpu = gpu();
    let pixels = checker(64);
    let scene = render_3d_with(
        json!({ "supersample": 1 }),
        vec![
            plate(10),
            plane(1, 10, json!({ "width": 4 })),
            ortho(2, json!({})),
            merge(3, &[1, 2], json!({})),
        ],
    );
    let frame = render(
        &mut gpu.renderer(),
        &scene.into(),
        0.0,
        &TestMedia::default().image("plate", 64, 64, pixels.clone()),
    );
    let worst = frame
        .rgba
        .iter()
        .zip(&pixels)
        .map(|(shown, texel)| shown.abs_diff(*texel))
        .max()
        .unwrap();
    assert!(
        worst <= 2,
        "a plane at one texel to a pixel is off by {worst}"
    );
}

#[test]
fn a_changing_raster_gets_mips_only_while_it_is_shown_smaller_than_it_is() {
    let gpu = gpu();
    // The steady passes a frame records with a raster that changes every frame, shown by a plane
    // `width` units across to an orthographic camera that sees 4 units, or from `distance` away
    // by a perspective one.
    let passes = |width: u32, distance: Option<f64>| {
        let camera = match distance {
            Some(z) => json!({ "id": 2, "type": "camera_3d", "params": { "position": [0, 0, z] } }),
            None => ortho(2, json!({})),
        };
        let composition: Composition = scene(
            (256, 256),
            json!([
                { "id": 10, "type": "background",
                  "params": { "paint": "=rgb(frame * 30, 99, 200)" } },
                plane(1, 10, json!({ "width": width })),
                camera,
                merge(3, &[1, 2], json!({})),
                { "id": 100, "type": "render_3d", "params": {}, "in": { "scene": 3 } },
            ]),
        )
        .frames(4)
        .into();
        let mut renderer = gpu.renderer();
        let media = TestMedia::default();
        (0..4)
            .map(|frame| {
                render(&mut renderer, &composition, f64::from(frame), &media);
                renderer.recorded_pass_count()
            })
            .last()
            .unwrap()
    };
    // A texel to a pixel, and half a texel to a pixel, take no more passes than one another.
    let unfiltered = passes(4, None);
    assert_eq!(passes(8, None), unfiltered);
    assert!(
        passes(8, Some(60.0)) >= unfiltered + 3,
        "a plane far away has mips made each frame"
    );
}

#[test]
fn mips_of_a_raster_that_stays_the_same_are_made_once() {
    let gpu = gpu();
    let passes = |source: Value, media: TestMedia| {
        let composition: Composition = scene(
            (256, 256),
            json!([
                source,
                plane(1, 10, json!({ "width": 8, "position": [0, 0, -30] })),
                { "id": 2, "type": "camera_3d",
                  "params": { "position": "=vec3(frame * 0.1, 0, 5)" } },
                merge(3, &[1, 2], json!({})),
                { "id": 100, "type": "render_3d", "params": {}, "in": { "scene": 3 } },
            ]),
        )
        .frames(4)
        .into();
        let mut renderer = gpu.renderer();
        (0..4)
            .map(|frame| {
                render(&mut renderer, &composition, f64::from(frame), &media);
                renderer.recorded_pass_count()
            })
            .collect::<Vec<_>>()
    };
    let image =
        json!({ "id": 10, "type": "media_in", "params": { "kind": "image", "source": "plate" } });
    let still = passes(
        image,
        TestMedia::default().image("plate", 512, 512, checker(512)),
    );
    assert!(
        still[0] > still[1],
        "the first frame makes the mips: {still:?}"
    );
    assert!(
        still[1..].iter().all(|&count| count == still[1]),
        "{still:?}"
    );
    let changing = json!({ "id": 10, "type": "background", "params": { "paint": "=rgb(frame * 30, 99, 200)" } });
    let changing = passes(changing, TestMedia::default());
    assert!(
        changing[1..].iter().all(|&count| count == changing[1]) && changing[1] > still[1],
        "a raster that changes has its mips made every frame: {changing:?} against {still:?}"
    );
}
