//! The size images are decoded at: reduced to fit the size limit, and to twice the size the
//! composition shows them at when every use bounds it.

use lumen_engine::composition::Composition;
use lumen_engine::diagnostic::Severity;
use lumen_engine::node::NodeId;
use serde_json::{Value, json};

use crate::support::{TestMedia, gpu, render, scene};

const RED: [u8; 4] = [255, 0, 0, 255];
const CLEAR: [u8; 4] = [0; 4];

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
    let composition = scene((32, 16), json!([photo_in()])).frames(2).into();
    let media = TestMedia::default().image("photo", 32, 16, RED.repeat(32 * 16));
    for frame in 0..2 {
        let shown = render(&mut renderer, &composition, f64::from(frame), &media);
        assert_eq!(shown.pixel(15, 7), RED);
        assert_eq!(shown.pixel(16, 8), CLEAR);
    }
    let warnings = renderer.take_diagnostics();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let warning = &warnings[0];
    assert_eq!(warning.code, "media.image_reduced");
    assert_eq!(warning.severity, Severity::Warning);
    assert_eq!(warning.node, Some(NodeId::new(1)));
    assert!(
        warning.message.contains("32x16") && warning.message.contains("16x8"),
        "{}",
        warning.message
    );
}

#[test]
fn an_image_wider_than_the_device_allows_renders_reduced() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let limit = renderer
        .gpu_renderer()
        .device
        .limits()
        .max_texture_dimension_2d;
    let width = limit + 1;
    let composition = scene((32, 16), json!([photo_in()])).into();
    let media = TestMedia::default().image("photo", width, 1, RED.repeat(width as usize));
    let shown = render(&mut renderer, &composition, 0.0, &media);
    assert_eq!(shown.pixel(0, 0), RED);
    assert_eq!(
        renderer.frame_graph_stats().adopted_bytes,
        u64::from(limit) * 4
    );
    let warnings = renderer.take_diagnostics();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].code, "media.image_reduced");
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
    assert_eq!(renderer.frame_graph_stats().adopted_bytes, 4000 * 3000 * 4);

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
    assert_eq!(renderer.frame_graph_stats().adopted_bytes, 400 * 320 * 4);
    assert_eq!(renderer.media_texture_upload_count(), 1);

    render(&mut renderer, &shown(400), 0.0, &media);
    assert_eq!(renderer.frame_graph_stats().adopted_bytes, 800 * 640 * 4);
    assert_eq!(renderer.media_texture_upload_count(), 2);

    // Shown at half its size or more, the image keeps its own.
    render(&mut renderer, &shown(500), 0.0, &media);
    assert_eq!(renderer.frame_graph_stats().adopted_bytes, 1000 * 800 * 4);
}

#[test]
fn an_image_a_transform_reads_keeps_its_own_size() {
    let gpu = gpu();
    let composition = shown_at(
        (400, 300),
        json!([
            photo_in(),
            { "id": 2, "type": "transform", "in": { "source": 1 },
              "params": { "scale_x": 0.1, "scale_y": 0.1 } },
        ]),
    );
    let mut renderer = gpu.renderer();
    render(&mut renderer, &composition, 0.0, &photo(2000, 1500));
    assert_eq!(renderer.frame_graph_stats().adopted_bytes, 2000 * 1500 * 4);
}
