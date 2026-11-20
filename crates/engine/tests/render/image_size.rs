//! The size images are decoded at: where only resizes, transforms and image planes read them,
//! reduced to fit the size limit, and to twice the size the composition shows them at.

use lumen_engine::composition::Composition;
use lumen_engine::diagnostic::Severity;
use lumen_engine::gpu::RenderTarget;
use lumen_engine::node::NodeId;
use serde_json::{Value, json};

use crate::support::{RED, TestMedia, gpu, render, scene};

/// A gradient smooth enough that sampling it at any scale gives the colour at the sample.
fn gradient(width: u32, height: u32) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            let (u, v) = (
                f64::from(x) / f64::from(width),
                f64::from(y) / f64::from(height),
            );
            let wave = 0.5 + 0.5 * (u * 6.0 + v * 4.0).sin();
            rgba.extend([u * 255.0, v * 255.0, wave * 255.0].map(|c| c.round() as u8));
            rgba.push(255);
        }
    }
    rgba
}

/// Bytes of a mip chain of `levels` Rgba16Float levels over a `width` by `height` level 0, which
/// a node shrinking its source adds to the textures adopted.
fn mip_bytes(width: u32, height: u32, levels: u32) -> u64 {
    (0..levels)
        .map(|level| u64::from((width >> level).max(1)) * u64::from((height >> level).max(1)) * 8)
        .sum()
}

fn photo(width: u32, height: u32) -> TestMedia {
    TestMedia::default().image("photo", width, height, gradient(width, height))
}

fn shown_at(canvas: (u32, u32), nodes: Value) -> Composition {
    scene(canvas, nodes).into()
}

fn resized(id: u32, width: Value, height: Value) -> Value {
    json!({ "id": id, "type": "resize", "in": { "source": 1 },
            "params": { "width": width, "height": height } })
}

fn photo_in() -> Value {
    json!({ "id": 1, "type": "media_in", "params": { "kind": "image", "source": "photo" } })
}

fn largest_channel_difference(a: &[u8], b: &[u8]) -> u8 {
    a.iter()
        .zip(b)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0)
}

#[test]
fn an_image_past_the_size_limit_renders_reduced_with_a_warning() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    renderer.set_max_image_side(16);
    let composition = scene(
        (32, 16),
        json!([photo_in(), resized(2, json!(32), json!(16))]),
    )
    .frames(2)
    .into();
    let media = TestMedia::default().image("photo", 32, 16, RED.repeat(32 * 16));
    for frame in 0..2 {
        let shown = render(&mut renderer, &composition, f64::from(frame), &media);
        assert_eq!(shown.pixel(0, 0), RED);
        assert_eq!(shown.pixel(31, 15), RED);
        if frame == 0 {
            assert_eq!(renderer.frame_graph_stats().adopted_bytes, 16 * 8 * 4);
        }
    }
    let warnings = renderer.take_diagnostics();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let warning = &warnings[0];
    assert_eq!(warning.code, "media.image_reduced");
    assert_eq!(warning.severity, Severity::Warning);
    assert_eq!(warning.node, Some(NodeId::new(1)));
    assert_eq!(warning.path, "/nodes/0");
    assert!(
        warning.message.contains("32x16") && warning.message.contains("16x8"),
        "{}",
        warning.message
    );
}

#[test]
fn an_image_past_the_size_limit_renders_reduced_through_an_animated_transform() {
    let gpu = gpu();
    // An expression's scale isn't known before rendering, so only the limit reduces the image.
    let composition = shown_at(
        (64, 32),
        json!([
            photo_in(),
            { "id": 2, "type": "transform", "in": { "source": 1 },
              "params": { "scale_x": "=0.5", "scale_y": "=0.5" } },
        ]),
    );
    let media = photo(64, 32);
    let mut renderer = gpu.renderer();
    let unlimited = render(&mut renderer, &composition, 0.0, &media);
    assert_eq!(renderer.frame_graph_stats().adopted_bytes, 64 * 32 * 4);

    let mut renderer = gpu.renderer();
    renderer.set_max_image_side(32);
    let limited = render(&mut renderer, &composition, 0.0, &media);
    assert_eq!(renderer.frame_graph_stats().adopted_bytes, 32 * 16 * 4);
    // Each pixel shows two by two texels of the full image, which the reduced one has averaged.
    let difference = largest_channel_difference(&limited.rgba, &unlimited.rgba);
    assert!(
        difference <= 3,
        "the reduced image is {difference} codes off"
    );
    let warnings = renderer.take_diagnostics();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].code, "media.image_reduced");
}

#[test]
fn a_transform_bounds_a_reduced_image_by_its_filtered_texels() {
    let gpu = gpu();
    // A 64x32 image under a limit of 4 has a texel to every 16 pixels, whose bilinear filtering
    // reaches 8 pixels past the image's edge.
    let composition = shown_at(
        (128, 64),
        json!([
            photo_in(),
            { "id": 2, "type": "transform", "in": { "source": 1 },
              "params": { "translate_x": 32, "translate_y": 16 } },
        ]),
    );
    let media = TestMedia::default().image("photo", 64, 32, RED.repeat(64 * 32));
    let shown = |full_frame: bool| {
        let mut renderer = gpu.renderer();
        renderer.set_max_image_side(4);
        renderer.set_full_frame_sources(full_frame);
        render(&mut renderer, &composition, 0.0, &media)
    };
    let (bounded, full) = (shown(false), shown(true));
    assert!(
        full.pixel(26, 32)[3] > 0,
        "the filtering reaches past the edge"
    );
    assert_eq!(bounded.rgba, full.rgba);
}

#[cfg(feature = "3d")]
#[test]
fn an_image_past_the_size_limit_renders_reduced_on_an_image_plane() {
    use crate::support::scene::{framed, plate, render_3d};
    let gpu = gpu();
    let media = crate::support::scene::gradient();
    let mut nodes = vec![plate(10)];
    nodes.extend(framed(10));
    let composition: Composition = render_3d(nodes).into();
    let mut renderer = gpu.renderer();
    let unlimited = render(&mut renderer, &composition, 0.0, &media);

    let mut renderer = gpu.renderer();
    renderer.set_max_image_side(16);
    let limited = render(&mut renderer, &composition, 0.0, &media);
    assert_eq!(renderer.cached_media_texture_count(), 1);
    let warnings = renderer.take_diagnostics();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].code, "media.image_reduced");
    assert!(
        warnings[0].message.contains("16x16"),
        "{}",
        warnings[0].message
    );
    // The plane keeps its shape, and a ramp sampled from a quarter of its texels stays near it.
    for (x, y) in [(0, 0), (63, 0), (0, 63), (63, 63), (32, 32)] {
        assert_eq!(
            limited.pixel(x, y)[3],
            unlimited.pixel(x, y)[3],
            "({x}, {y})"
        );
    }
    // Magnified 4 times, the reduced ramp is interpolated in linear light across 16 codes a texel.
    let difference = largest_channel_difference(&limited.rgba, &unlimited.rgba);
    assert!(
        difference <= 8,
        "the reduced plane is {difference} codes off"
    );
}

#[test]
fn an_image_past_the_device_limit_that_a_blur_reads_fails() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let limit = renderer
        .gpu_renderer()
        .device
        .limits()
        .max_texture_dimension_2d;
    let width = limit * 2;
    let composition = shown_at(
        (32, 16),
        json!([
            photo_in(),
            resized(2, json!(32), json!(16)),
            { "id": 3, "type": "blur", "in": { "source": 1 } },
            { "id": 4, "type": "merge", "in": { "base": 2, "overlay": 3 } },
        ]),
    );
    let media = TestMedia::default().image("photo", width, 2, RED.repeat(width as usize * 2));
    let error = renderer
        .render(&composition, 0.0, &media, RenderTarget::Owned)
        .unwrap_err();
    assert_eq!(error.code(), "media.too_large");
    let message = error.to_string();
    assert!(
        message.contains("`resize`, `transform` and `image_plane_3d`"),
        "{message}"
    );
}

#[test]
fn an_image_is_reduced_on_each_axis_whatever_its_aspect_ratio() {
    let gpu = gpu();
    // 1000 × 9 fits 200 × 1.8 here, so it is kept at 400 × 4, though no size that small has its
    // aspect ratio.
    let fit = |after: Value| {
        shown_at(
            (200, 200),
            json!([
                photo_in(),
                { "id": 2, "type": "resize", "in": { "source": 1 },
                  "params": { "width": 200, "height": 200, "mode": "fit" } },
                after,
            ]),
        )
    };
    let media = photo(1000, 9);
    let mut renderer = gpu.renderer();
    let bounded = render(
        &mut renderer,
        &fit(json!({ "id": 3, "type": "opacity", "in": { "source": 2 } })),
        0.0,
        &media,
    );
    assert_eq!(renderer.frame_graph_stats().adopted_bytes, 400 * 4 * 4);

    let mut renderer = gpu.renderer();
    let unbounded = render(
        &mut renderer,
        &fit(json!({ "id": 3, "type": "transform", "in": { "source": 2 },
                     "params": { "scale_x": "=1", "scale_y": "=1" } })),
        0.0,
        &media,
    );
    // The strip crosses rows 99 and 100, and each pixel averages about 5 by 5 texels of the full
    // image or 2 by 2 of the reduced one.
    assert!(bounded.pixel(100, 99)[3] > 0 && bounded.pixel(100, 100)[3] > 0);
    let difference = largest_channel_difference(&bounded.rgba, &unbounded.rgba);
    assert!(
        difference <= 3,
        "the reduced image is {difference} codes off"
    );
}

#[test]
fn an_image_wider_than_the_device_allows_renders_reduced_through_a_resize() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let limit = renderer
        .gpu_renderer()
        .device
        .limits()
        .max_texture_dimension_2d;
    let width = limit * 2;
    let composition = shown_at(
        (32, 16),
        json!([photo_in(), resized(2, json!(32), json!(16))]),
    );
    let media = TestMedia::default().image("photo", width, 2, RED.repeat(width as usize * 2));
    let shown = render(&mut renderer, &composition, 0.0, &media);
    assert_eq!(shown.pixel(0, 0), RED);
    assert_eq!(shown.pixel(31, 15), RED);
    assert_eq!(
        renderer.frame_graph_stats().adopted_bytes,
        u64::from(limit) * 4 + mip_bytes(limit, 1, 6)
    );
    let warnings = renderer.take_diagnostics();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].code, "media.image_reduced");
}

#[test]
fn warnings_of_reduced_images_stop_at_32_sources() {
    let gpu = gpu();
    let mut nodes = Vec::new();
    let mut layers = Vec::new();
    let mut media = TestMedia::default();
    for index in 0..40u32 {
        let source = format!("photo{index}");
        let (image, resize) = (index * 2 + 1, index * 2 + 2);
        nodes.push(json!({ "id": image, "type": "media_in",
                           "params": { "kind": "image", "source": source } }));
        nodes.push(
            json!({ "id": resize, "type": "resize", "in": { "source": image },
                           "params": { "width": 4, "height": 2 } }),
        );
        layers.push(resize);
        media = media.image(&source, 4, 2, RED.repeat(8));
    }
    nodes.push(json!({ "id": 100, "type": "raster_multimerge", "in": { "layers": layers } }));
    let composition = shown_at((4, 2), Value::Array(nodes));
    let mut renderer = gpu.renderer();
    renderer.set_max_image_side(2);
    let shown = render(&mut renderer, &composition, 0.0, &media);
    assert_eq!(shown.pixel(3, 1), RED);
    // Every image is reduced, to 2 × 1, and only the first 32 warn.
    assert_eq!(renderer.frame_graph_stats().adopted_bytes, 40 * 2 * 4);
    let warnings = renderer.take_diagnostics();
    assert_eq!(warnings.len(), 32);
    assert!(warnings.iter().all(|w| w.code == "media.image_reduced"));
}

#[test]
fn an_image_shown_at_a_tenth_by_a_literal_resize_is_decoded_at_a_fifth() {
    let gpu = gpu();
    let media = photo(4000, 3000);
    let literal = shown_at(
        (400, 300),
        json!([photo_in(), resized(2, json!(400), json!(300))]),
    );
    let mut renderer = gpu.renderer();
    let reduced = render(&mut renderer, &literal, 0.0, &media);
    assert_eq!(renderer.frame_graph_stats().adopted_bytes, 800 * 600 * 4);

    // A scale an expression sets could magnify the image, which keeps it at its own size.
    let scaled = shown_at(
        (400, 300),
        json!([
            photo_in(),
            resized(2, json!(400), json!(300)),
            { "id": 3, "type": "transform", "in": { "source": 2 },
              "params": { "scale_x": "=1", "scale_y": "=1" } },
        ]),
    );
    let mut renderer = gpu.renderer();
    let native = render(&mut renderer, &scaled, 0.0, &media);
    assert_eq!(
        renderer.frame_graph_stats().adopted_bytes,
        4000 * 3000 * 4 + mip_bytes(4000, 3000, 5)
    );

    let difference = largest_channel_difference(&reduced.rgba, &native.rgba);
    assert!(
        difference <= 2,
        "the reduced image is {difference} codes off"
    );
}

#[test]
fn uses_of_one_image_take_the_largest_and_a_larger_one_decodes_it_again() {
    let gpu = gpu();
    let media = photo(1000, 800);
    let shown = |larger: u32| {
        shown_at(
            (400, 320),
            json!([
                photo_in(),
                resized(2, json!(100), json!(80)),
                { "id": 3, "type": "media_in", "params": { "kind": "image", "source": "photo" } },
                { "id": 4, "type": "resize", "in": { "source": 3 },
                  "params": { "width": larger, "height": larger * 4 / 5 } },
                { "id": 5, "type": "merge", "in": { "base": 4, "overlay": 2 } },
            ]),
        )
    };
    let mut renderer = gpu.renderer();
    render(&mut renderer, &shown(200), 0.0, &media);
    // Shown at a quarter or more, a use reads the image itself, with no chain.
    assert_eq!(renderer.frame_graph_stats().adopted_bytes, 400 * 320 * 4);
    assert_eq!(renderer.media_texture_upload_count(), 1);

    render(&mut renderer, &shown(400), 0.0, &media);
    assert_eq!(
        renderer.frame_graph_stats().adopted_bytes,
        800 * 640 * 4 + mip_bytes(800, 640, 4)
    );
    assert_eq!(renderer.media_texture_upload_count(), 2);

    // Shown at half its size or more, the image keeps its own.
    render(&mut renderer, &shown(500), 0.0, &media);
    assert_eq!(
        renderer.frame_graph_stats().adopted_bytes,
        // The chain of the image decoded before stays until it has been idle for a while.
        1000 * 800 * 4 + mip_bytes(1000, 800, 5) + mip_bytes(800, 640, 4)
    );
}

#[test]
fn an_image_a_literal_transform_shows_at_a_quarter_is_decoded_at_half() {
    let gpu = gpu();
    let media = photo(800, 600);
    let quarter = |scale: Value| {
        shown_at(
            (800, 600),
            json!([
                photo_in(),
                { "id": 2, "type": "transform", "in": { "source": 1 },
                  "params": { "scale_x": scale, "scale_y": scale } },
            ]),
        )
    };
    let mut renderer = gpu.renderer();
    let reduced = render(&mut renderer, &quarter(json!(0.25)), 0.0, &media);
    assert_eq!(renderer.cached_media_texture_count(), 1);
    assert_eq!(renderer.frame_graph_stats().adopted_bytes, 400 * 300 * 4);

    // The same scale from an expression keeps the image at its own size.
    let mut renderer = gpu.renderer();
    let native = render(&mut renderer, &quarter(json!("=0.25")), 0.0, &media);
    assert_eq!(renderer.frame_graph_stats().adopted_bytes, 800 * 600 * 4);

    // Both shrink about the image's centre, into 200 × 150 pixels from (300, 225).
    for frame in [&reduced, &native] {
        assert_eq!(frame.pixel(299, 300)[3], 0);
        assert_eq!(frame.pixel(300, 300)[3], 255);
        assert_eq!(frame.pixel(499, 374)[3], 255);
        assert_eq!(frame.pixel(500, 374)[3], 0);
        assert_eq!(frame.pixel(400, 375)[3], 0);
    }
    // Each pixel averages four by four texels of the full image, or two by two of the reduced one,
    // which has averaged two by two of the full image's in linear light.
    let difference = largest_channel_difference(&reduced.rgba, &native.rgba);
    assert!(
        difference <= 2,
        "the reduced image is {difference} codes off"
    );
}
