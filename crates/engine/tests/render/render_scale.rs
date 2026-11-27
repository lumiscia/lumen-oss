//! Rendering at a fraction of the composition's size: frames look like the full render scaled,
//! every size rounds up, and a renderer moves between scales without showing stale output.

use lumen_engine::composition::Composition;
use lumen_engine::gpu::GpuCompositionRenderer;
use serde_json::{Value, json};

use crate::support::{
    Frame, TestMedia, check_each, filled_and_stroked, filled_path, filled_shape, gpu, render, scene,
};

const CANVAS: (u32, u32) = (96, 64);

fn scaled(renderer: &mut GpuCompositionRenderer, scale: f32) -> &mut GpuCompositionRenderer {
    renderer.set_render_scale(scale).expect("a valid scale");
    renderer
}

/// Ramps under a checker of 8-pixel squares, which a half-size raster still resolves.
fn photo() -> Vec<u8> {
    (0..32 * 32)
        .flat_map(|index| {
            let (x, y) = (index % 32, index / 32);
            [
                x * 8,
                y * 8,
                if (x / 8 + y / 8) % 2 == 0 { 255 } else { 40 },
                255,
            ]
            .map(|channel| channel.min(255) as u8)
        })
        .collect()
}

fn media() -> TestMedia {
    crate::text_pixels::media().image("photo", 32, 32, photo())
}

/// Layers of every kind whose lengths are in canvas pixels: a stroked shape with a user-space
/// gradient under a blur, a stroked path, text, and a cropped, resized and moved image with a
/// shadow, over a background.
fn layered() -> Composition {
    let gradient = json!({
        "type": "linear_gradient", "units": "user_space", "start": [8, 0], "end": [48, 0],
        "stops": [{ "offset": 0, "color": [255, 40, 0, 255] }, { "offset": 1, "color": [0, 80, 255, 255] }]
    });
    scene(
        CANVAS,
        json!([
            { "id": 1, "type": "background", "params": { "paint": [30, 40, 50, 255] } },
            filled_and_stroked(
                2,
                "shape_2d",
                json!({ "width": 40, "height": 24, "border_radius": 6, "position": [8, 8] }),
                json!({ "paint": gradient }),
                json!({ "width": 2, "paint": [255, 255, 255, 255] }),
            ),
            { "id": 3, "type": "blur", "in": { "source": 2 }, "params": { "radius": 2 } },
            filled_and_stroked(
                4,
                "path_2d",
                json!({ "data": "M 0 0 L 30 0 L 15 20 Z", "position": [56, 6] }),
                json!({ "paint": [40, 220, 90, 255] }),
                json!({ "width": 1 }),
            ),
            { "id": 5, "type": "text", "params": {
                "content": "Ag", "font_family": "Inter", "font_size": 20,
                "position": [10, 36], "color": [255, 255, 255, 255] } },
            { "id": 6, "type": "media_in", "params": { "kind": "image", "source": "photo" } },
            { "id": 7, "type": "crop", "in": { "source": 6 },
              "params": { "x": 4, "y": 4, "width": 24, "height": 24 } },
            { "id": 8, "type": "resize", "in": { "source": 7 },
              "params": { "width": 24, "height": 24 } },
            { "id": 9, "type": "transform", "in": { "source": 8 },
              "params": { "translate_x": 60, "translate_y": 34 } },
            { "id": 10, "type": "shadow", "in": { "source": 9 },
              "params": { "radius": 3, "offset_x": 2, "offset_y": 2, "opacity": 0.8 } },
            { "id": 11, "type": "raster_multimerge", "in": { "layers": [1, 3, 4, 5, 10] } },
        ]),
    )
    .into()
}

fn to_linear(code: u8) -> f64 {
    let value = f64::from(code) / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn to_code(linear: f64) -> u8 {
    let value = if linear <= 0.0031308 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    };
    (value * 255.0).round().clamp(0.0, 255.0) as u8
}

/// `frame` shrunk by a whole `factor`, each pixel the average in linear light of the pixels it
/// covers. Every frame here is opaque.
fn area_downscaled(frame: &Frame, factor: u32) -> Frame {
    let (width, height) = (frame.width / factor, frame.height / factor);
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let mut sum = [0.0; 3];
            for (dx, dy) in (0..factor).flat_map(|dx| (0..factor).map(move |dy| (dx, dy))) {
                let pixel = frame.pixel(x * factor + dx, y * factor + dy);
                assert_eq!(pixel[3], 255, "the layered scene is opaque");
                for (sum, code) in sum.iter_mut().zip(pixel) {
                    *sum += to_linear(code);
                }
            }
            let count = f64::from(factor * factor);
            rgba.extend(sum.map(|sum| to_code(sum / count)));
            rgba.push(255);
        }
    }
    Frame {
        at: frame.at,
        width,
        height,
        rgba,
    }
}

/// The mean and 99th percentile of the channel differences between two frames of one size.
fn differences(a: &Frame, b: &Frame) -> (f64, u8) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let mut differences: Vec<u8> = a
        .rgba
        .iter()
        .zip(&b.rgba)
        .map(|(a, b)| a.abs_diff(*b))
        .collect();
    let mean = differences.iter().map(|&d| f64::from(d)).sum::<f64>() / differences.len() as f64;
    differences.sort_unstable();
    (mean, differences[differences.len() * 99 / 100])
}

#[test]
fn half_scale_looks_like_the_full_render_shrunk() {
    let gpu = gpu();
    let composition = layered();
    let media = media();
    let full = render(&mut gpu.renderer(), &composition, 0.0, &media);
    let half = render(scaled(&mut gpu.renderer(), 0.5), &composition, 0.0, &media);
    assert_eq!((half.width, half.height), (48, 32));

    let expected = area_downscaled(&full, 2);
    let (mean, p99) = differences(&half, &expected);
    // Measured at a mean of 1.4 and a 99th percentile of 25 codes. Glyphs rasterized at 10 px
    // rather than averaged from 20 px differ by up to about 55 codes at a few stem pixels, and
    // edges antialiased over one device pixel rather than averaged from four sharper ones by up
    // to about 45 where a 1 px stroke turns half a device pixel wide. A layer a device pixel off
    // would move the mean past this, and away from edges the two agree to rounding.
    assert!(mean < 2.0 && p99 <= 32, "mean {mean:.3}, p99 {p99}");

    let close = |a: [u8; 4], b: [u8; 4]| a.iter().zip(&b).all(|(a, b)| a.abs_diff(*b) <= 2);
    // The background, the middle of the shape and its gradient, the path, and the image all sit
    // where the full render puts them.
    for (x, y) in [(1, 30), (14, 10), (20, 10), (36, 6), (36, 22), (42, 26)] {
        assert!(
            close(half.pixel(x, y), expected.pixel(x, y)),
            "({x}, {y}): {:?} against {:?}",
            half.pixel(x, y),
            expected.pixel(x, y)
        );
    }
}

/// A shader that writes its resolution and render scale as sRGB codes.
const GLOBALS_SHADER: &str = "
@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let codes = vec3<f32>(lumen.resolution, lumen.render_scale * 100.0) / 255.0;
    textureStore(output, vec2<i32>(id.xy), vec4<f32>(lumen_srgb_to_linear(codes), 1.0));
}
";

/// A `render_3d` of its own size, `supersample` above it, which the globals shader reports.
fn render_3d_sized() -> Value {
    json!([
        { "id": 1, "type": "material_3d", "params": { "unlit": true, "color": [255, 0, 0, 255] } },
        { "id": 2, "type": "shape_3d", "in": { "material": 1 }, "params": {} },
        { "id": 3, "type": "render_3d", "in": { "scene": 2 },
          "params": { "width": 40, "height": 21, "supersample": 2 } },
        { "id": 4, "type": "wgsl_shader", "in": { "source": [3] },
          "params": { "shader": GLOBALS_SHADER, "uniforms": {} } },
    ])
}

/// Nodes, canvas, scale, the frame size, and the codes the globals shader writes, if it runs.
type SizeRow = (Value, (u32, u32), f32, (u32, u32), Option<[u8; 3]>);

#[cfg(feature = "3d")]
#[test]
fn sizes_round_up_and_shaders_see_device_pixels() {
    let gpu = gpu();
    let rows: Vec<(&str, SizeRow)> = vec![
        (
            "odd canvas",
            (
                json!([{ "id": 1, "type": "background", "params": { "paint": [9, 9, 9, 255] } }]),
                (101, 51),
                0.5,
                (51, 26),
                None,
            ),
        ),
        (
            "shader globals",
            (
                json!([
                    { "id": 1, "type": "background", "params": { "paint": [0, 0, 0, 255] } },
                    { "id": 2, "type": "wgsl_shader", "in": { "source": [1] },
                      "params": { "shader": GLOBALS_SHADER, "uniforms": {} } },
                ]),
                (24, 16),
                0.5,
                (12, 8),
                Some([12, 8, 50]),
            ),
        ),
        (
            "render_3d size",
            (
                render_3d_sized(),
                (64, 64),
                0.5,
                (32, 32),
                Some([20, 11, 50]),
            ),
        ),
    ];
    check_each(rows, |(nodes, canvas, scale, size, codes)| {
        let composition: Composition = scene(canvas, nodes).into();
        let mut renderer = gpu.renderer();
        let output = scaled(&mut renderer, scale).output_size(&composition);
        assert_eq!((output.width, output.height), size);
        let frame = render(&mut renderer, &composition, 0.0, &TestMedia::default());
        assert_eq!((frame.width, frame.height), size);
        if let Some(codes) = codes {
            assert_eq!(frame.pixel(0, 0)[..3], codes);
        }
    });
}

#[test]
fn a_renderer_moves_between_scales_without_stale_output() {
    let gpu = gpu();
    let composition = layered();
    let media = media();
    let mut renderer = gpu.renderer();
    let full = render(&mut renderer, &composition, 0.0, &media);

    let bound = renderer
        .bind_frame(
            &composition,
            0.0,
            &media,
            lumen_engine::gpu::RenderTarget::Owned,
        )
        .expect("binds");
    scaled(&mut renderer, 0.5);
    assert!(
        renderer.upload_frame(&bound).is_err(),
        "a frame bound before the change is stale"
    );

    let half = render(&mut renderer, &composition, 0.0, &media);
    let fresh = render(scaled(&mut gpu.renderer(), 0.5), &composition, 0.0, &media);
    assert_eq!((half.width, half.height), (48, 32));
    assert!(
        half.rgba == fresh.rgba,
        "half scale matches a new renderer's"
    );

    let bound = renderer
        .bind_frame(
            &composition,
            0.0,
            &media,
            lumen_engine::gpu::RenderTarget::Owned,
        )
        .expect("binds");
    scaled(&mut renderer, 0.5);
    assert!(
        renderer.upload_frame(&bound).is_ok(),
        "the same scale changes nothing"
    );
    renderer.submit_frame(&bound).expect("submits");

    scaled(&mut renderer, 1.0);
    let again = render(&mut renderer, &composition, 0.0, &media);
    assert!(
        again.rgba == full.rgba,
        "back at full scale, frames are as before"
    );
}

/// How many pixels of `frame` have any alpha.
fn covered(frame: &Frame) -> usize {
    let pixels = frame.rgba.as_chunks::<4>().0;
    pixels.iter().filter(|pixel| pixel[3] > 0).count()
}

/// Nodes, scale, and the check of the frame they render.
type ProbeRow = (Value, f32, fn(&Frame));

#[test]
fn rectangles_keep_their_scaled_edges_and_thin_shapes_stay_thin() {
    let gpu = gpu();
    let solid =
        json!({ "id": 1, "type": "background", "params": { "paint": [255, 255, 255, 255] } });
    let rows: Vec<(&str, ProbeRow)> = vec![
        (
            // The right edge, 4, is exactly 2 device pixels.
            "crop edges scale together",
            (
                json!([solid, { "id": 2, "type": "crop", "in": { "source": 1 },
                    "params": { "x": 1, "y": 1, "width": 3, "height": 3 } }]),
                0.5,
                |frame| assert_eq!(covered(frame), 1),
            ),
        ),
        (
            "a shape one canvas pixel wide covers half a device pixel",
            (
                json!([filled_shape(
                    1,
                    json!({ "width": 1, "height": 8, "position": [4, 4] }),
                    json!({ "paint": [255, 255, 255, 255] })
                )]),
                0.5,
                |frame| {
                    let alpha = frame.pixel(2, 3)[3];
                    assert!((120..=136).contains(&alpha), "alpha {alpha}");
                    assert_eq!(frame.pixel(3, 3)[3], 0);
                },
            ),
        ),
        (
            "a thin path keeps its scaled width",
            (
                json!([filled_path(
                    1,
                    json!({ "data": "M 0 0 L 1 0 L 1 8 L 0 8 Z", "position": [4, 4] }),
                    json!({ "paint": [255, 255, 255, 255] })
                )]),
                0.5,
                |frame| {
                    let alpha = frame.pixel(2, 3)[3];
                    assert!((120..=136).contains(&alpha), "alpha {alpha}");
                    assert_eq!(frame.pixel(3, 3)[3], 0);
                },
            ),
        ),
    ];
    check_each(rows, |(nodes, scale, check)| {
        let composition: Composition = scene((8, 8), nodes).into();
        let frame = render(
            scaled(&mut gpu.renderer(), scale),
            &composition,
            0.0,
            &TestMedia::default(),
        );
        check(&frame);
    });
}

#[test]
fn a_blur_past_the_cap_looks_the_same_at_every_scale() {
    let gpu = gpu();
    let blurred = |radius: u32| -> Composition {
        scene(
            CANVAS,
            json!([
                filled_shape(1, json!({ "width": 90, "height": 60, "position": [3, 2] }), json!({ "paint": [255, 255, 255, 255] })),
                { "id": 2, "type": "blur", "in": { "source": 1 }, "params": { "radius": radius } },
            ]),
        )
        .into()
    };
    let media = TestMedia::default();
    let mut renderer = gpu.renderer();
    let frames: Vec<_> = [700, 1400]
        .map(|radius| render(scaled(&mut renderer, 0.5), &blurred(radius), 0.0, &media))
        .into();
    assert!(frames[0].rgba == frames[1].rgba, "radii past the cap match");
}

#[cfg(feature = "3d")]
#[test]
fn shadow_maps_shrink_with_the_render_scale() {
    use crate::support::scene::{GroundView, light};

    let gpu = gpu();
    let composition: Composition = GroundView::WHOLE
        .scene(
            vec![
                light(
                    60,
                    json!({ "light": "directional", "intensity": 3, "cast_shadows": true,
                        "rotation": [-45, -90, 0] }),
                ),
                json!({ "id": 1, "type": "shape_3d", "params": {
                    "size": [0.6, 0.2, 0.6], "position": [-1, 2, 0] } }),
            ],
            json!({}),
        )
        .into();
    let held = |scale: f32| {
        let mut renderer = gpu.renderer();
        render(
            scaled(&mut renderer, scale),
            &composition,
            0.0,
            &TestMedia::default(),
        );
        renderer.frame_graph_stats().recorder.peak_frame_bytes
    };
    let (full, half) = (held(1.0), held(0.5));
    // A 4096 square depth atlas is 64 MiB, and a 2048 square one 16 MiB.
    assert!(full - half >= 32 << 20, "{full} bytes at 1, {half} at 0.5");
}
