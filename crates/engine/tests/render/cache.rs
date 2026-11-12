//! Output kept between frames: what static layers keep, and how it survives switches between
//! layers, edits, external targets and the shared text atlas.

use lumen_engine::{
    composition::Composition,
    gpu::{GpuCompositionRenderer, RenderTarget},
};
use lumen_gpu::wgpu;
use serde_json::{Value, json};

use crate::support::{Scene, TestMedia, gpu, read_rgba8, render, scene};

const CANVAS: (u32, u32) = (32, 16);

fn canvas(nodes: Vec<Value>) -> Scene {
    scene(CANVAS, json!(nodes)).frames(12)
}

fn background(id: u32, color: [u8; 4]) -> Value {
    json!({ "id": id, "type": "background", "params": {
        "paint": color, "width": CANVAS.0, "height": CANVAS.1
    } })
}

fn square(id: u32, position: Value, size: u32, color: [u8; 4]) -> Value {
    json!({ "id": id, "type": "shape", "params": {
        "position": position, "width": size, "height": size, "fill_paint": color
    } })
}

fn merge(id: u32, layers: &[u32]) -> Value {
    json!({ "id": id, "type": "raster_multimerge", "in": { "layers": layers } })
}

fn switch(id: u32, selected_layer: &str, layers: &[u32]) -> Value {
    json!({ "id": id, "type": "switch", "params": { "selected_layer": format!("={selected_layer}") },
        "in": { "layers": layers } })
}

/// A static layer (node 3) under a square moving with the frame.
fn static_under_animated() -> Scene {
    canvas(vec![
        background(1, [200, 30, 30, 255]),
        square(2, json!([4, 4]), 8, [20, 40, 220, 255]),
        merge(3, &[1, 2]),
        square(4, json!("=vec2(frame * 2, 6)"), 4, [30, 220, 60, 255]),
        merge(5, &[3, 4]),
    ])
}

fn at(renderer: &mut GpuCompositionRenderer, composition: &Composition, frame: f64) -> Vec<u8> {
    render(renderer, composition, frame, &TestMedia::default()).rgba
}

/// Renders `frame` without any output kept from earlier frames.
fn uncached(
    renderer: &mut GpuCompositionRenderer,
    composition: &Composition,
    frame: f64,
) -> Vec<u8> {
    renderer.release_resources();
    at(renderer, composition, frame)
}

fn read_raster(
    renderer: &GpuCompositionRenderer,
    raster: lumen_engine::gpu::RasterHandle,
) -> Vec<u8> {
    read_rgba8(
        renderer.gpu_renderer(),
        &raster.texture,
        raster.domain.storage_size,
    )
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = (y * CANVAS.0 + x) as usize * 4;
    pixels[at..at + 4].try_into().unwrap()
}

fn nonblank(pixels: &[u8]) -> bool {
    pixels.iter().any(|&byte| byte > 0)
}

#[test]
fn a_zero_budget_keeps_what_a_bound_frame_renders_with() {
    let gpu = gpu();
    let (mut renderer, mut reference) = (gpu.renderer(), gpu.renderer());
    let media = TestMedia::default();
    let composition: Composition = static_under_animated().into();
    assert!(nonblank(&at(&mut renderer, &composition, 0.0)));
    let bound = renderer
        .bind_frame(&composition, 1.0, &media, RenderTarget::Owned)
        .unwrap();
    renderer.set_cache_budgets(lumen_engine::gpu::CacheBudgets {
        pooled_textures: 0,
        ..renderer.cache_budgets()
    });
    renderer.upload_frame(&bound).unwrap();
    let raster = renderer.submit_frame(&bound).unwrap().raster;
    let pixels = read_raster(&renderer, raster);
    assert!(pixels == uncached(&mut reference, &composition, 1.0));
    assert_eq!(pixel(&pixels, 20, 2), [200, 30, 30, 255]);
    assert_eq!(pixel(&pixels, 8, 11), [20, 40, 220, 255]);
    assert_eq!(pixel(&pixels, 3, 7), [30, 220, 60, 255]);
}

/// A `tile` template of two canvas-wide halves, whose node ids repeat the root's, and one or two
/// clips of it, the second moved to the right half.
fn tile_clips(clips: Value, template_nodes: Value) -> Composition {
    let mut nodes = clips.as_array().unwrap().clone();
    let mut connections = vec![json!({ "from_node": 1, "to_node": 5, "to_port": "source" })];
    if nodes.len() == 2 {
        nodes.extend([
            json!({ "id": 3, "type": "transform", "params": { "translate_x": 2 } }),
            json!({ "id": 4, "type": "merge" }),
        ]);
        connections = vec![
            json!({ "from_node": 2, "to_node": 3, "to_port": "source" }),
            json!({ "from_node": 1, "to_node": 4, "to_port": "base" }),
            json!({ "from_node": 3, "to_node": 4, "to_port": "overlay" }),
            json!({ "from_node": 4, "to_node": 5, "to_port": "source" }),
        ];
    }
    nodes.push(json!({ "id": 5, "type": "media_output" }));
    lumen_engine::json::parse_value(&json!({
        "timeline": { "fps": 10, "duration_frames": 20 },
        "render_settings": { "width": 4, "height": 1 },
        "compositions": {
            "tile": {
                "render_settings": { "width": 2, "height": 1 },
                "timeline": { "fps": 10, "duration_frames": 20 },
                "props": { "g": { "type": "number", "default": 0 } },
                "nodes": template_nodes,
                "connections": [
                    { "from_node": 1, "to_node": 2, "to_port": "source" },
                    { "from_node": 2, "to_node": 4, "to_port": "base" },
                    { "from_node": 3, "to_node": 4, "to_port": "overlay" },
                    { "from_node": 4, "to_node": 5, "to_port": "source" }
                ]
            }
        },
        "nodes": nodes,
        "connections": connections
    }))
    .unwrap()
}

#[test]
fn static_template_parts_are_kept_per_clip_instance() {
    let template = json!([
        { "id": 1, "type": "background", "params": { "paint": "#2040c0" } },
        { "id": 2, "type": "opacity", "params": { "opacity": 0.5 } },
        { "id": 3, "type": "background", "params": { "paint": "=rgba(frame * 10, prop('g'), 0, 128)" } },
        { "id": 4, "type": "merge" },
        { "id": 5, "type": "media_output" }
    ]);
    let first = json!({ "id": 1, "type": "clip", "params": { "composition": "tile", "props": { "g": 40 } } });
    let second = json!({ "id": 2, "type": "clip", "params": { "composition": "tile", "start": 2, "props": { "g": 200 } } });
    let both = tile_clips(json!([first, second]), template.clone());
    let one = tile_clips(json!([first]), template);
    let gpu = gpu();
    let (mut renderer, mut reference) = (gpu.renderer(), gpu.renderer());
    let mut kept_per_frame = |composition: &Composition| {
        let mut cached = Vec::new();
        let mut frames = Vec::new();
        for frame in 2..6 {
            let pixels = at(&mut renderer, composition, f64::from(frame));
            assert!(
                pixels == uncached(&mut reference, composition, f64::from(frame)),
                "frame {frame} differs from an uncached render"
            );
            cached.push(renderer.skipped_pass_count());
            frames.push(pixels);
        }
        let per_frame = cached[1] - cached[0];
        let start = cached[0];
        assert_eq!(
            cached,
            [
                start,
                start + per_frame,
                start + 2 * per_frame,
                start + 3 * per_frame
            ],
            "the same passes are kept every frame"
        );
        assert!(frames[0] != frames[3], "the animated layer still moves");
        (per_frame, frames)
    };
    let (one_kept, _) = kept_per_frame(&one);
    let (both_kept, frames) = kept_per_frame(&both);
    assert!(one_kept >= 2, "the background and its opacity are kept");
    assert_eq!(
        both_kept,
        2 * one_kept,
        "each clip keeps its own static part"
    );
    assert!(
        frames[2][..8] != frames[2][8..],
        "the clips show their own props and time"
    );
}

/// Static layers 4 and 5 sharing square 3, and 7 with a third colour, selected in turn every two
/// frames, under a moving square.
fn switching_static_layers() -> Composition {
    canvas(vec![
        background(1, [200, 30, 30, 255]),
        background(2, [30, 200, 30, 255]),
        background(8, [30, 30, 200, 255]),
        square(3, json!([4, 4]), 8, [255, 255, 255, 255]),
        merge(4, &[1, 3]),
        merge(5, &[2, 3]),
        merge(9, &[8, 3]),
        switch(6, "floor(frame / 2) % 3", &[4, 5, 9]),
        square(7, json!("=vec2(frame * 2, 6)"), 4, [20, 40, 220, 255]),
        merge(10, &[6, 7]),
    ])
    .into()
}

/// What `switching_static_layers` shows at `frame`, away from its moving square.
fn assert_switched_layer(pixels: &[u8], frame: u32) {
    let color =
        [[200, 30, 30, 255], [30, 200, 30, 255], [30, 30, 200, 255]][frame as usize / 2 % 3];
    assert_eq!(pixel(pixels, 28, 14), color, "frame {frame}");
    assert_eq!(pixel(pixels, 8, 11), [255, 255, 255, 255], "frame {frame}");
}

#[test]
fn kept_output_survives_switches_and_is_dropped_on_release() {
    let gpu = gpu();
    let mut reference = gpu.renderer();
    let composition = switching_static_layers();
    // Frames 6 and 7 select the first layer again.
    for budget in [None, Some(0)] {
        let mut renderer = gpu.renderer();
        if let Some(budget) = budget {
            renderer.set_cache_budgets(lumen_engine::gpu::CacheBudgets {
                pooled_textures: budget,
                ..renderer.cache_budgets()
            });
        }
        for frame in 0..8 {
            let pixels = at(&mut renderer, &composition, f64::from(frame));
            let expected = uncached(&mut reference, &composition, f64::from(frame));
            assert!(pixels == expected, "frame {frame} with budget {budget:?}");
            assert_switched_layer(&pixels, frame);
        }
        assert!(renderer.skipped_pass_count() > 0, "budget {budget:?}");
        assert!(renderer.frame_graph_stats().retained_bytes > 0);
        renderer.release_resources();
        assert_eq!(renderer.frame_graph_stats().retained_bytes, 0);
    }
}

#[test]
fn a_layer_past_the_retained_budget_renders_again_when_revisited() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let composition = switching_static_layers();
    at(&mut renderer, &composition, 0.0);
    let budget = renderer.frame_graph_stats().retained_bytes;
    assert!(budget > 0);
    renderer.set_cache_budgets(lumen_engine::gpu::CacheBudgets {
        retained_outputs: budget,
        ..renderer.cache_budgets()
    });
    for frame in 1..6 {
        let pixels = at(&mut renderer, &composition, f64::from(frame));
        assert_switched_layer(&pixels, frame);
        assert!(
            renderer.frame_graph_stats().retained_bytes <= budget,
            "frame {frame}"
        );
    }

    // Frames 6 and 7 select the first layer, which frame 6 has to draw again and frame 7 doesn't.
    assert_switched_layer(&at(&mut renderer, &composition, 6.0), 6);
    let redrawn = renderer.recorded_pass_count();
    assert_switched_layer(&at(&mut renderer, &composition, 7.0), 7);
    assert!(redrawn > renderer.recorded_pass_count());
}

#[test]
fn discarded_binds_keep_nothing_and_stale_uploads_are_still_rejected() {
    let gpu = gpu();
    let (mut renderer, mut reference) = (gpu.renderer(), gpu.renderer());
    let mut composition = static_under_animated();
    composition.0["nodes"][1] = json!({ "id": 2, "type": "text", "params": {
        "content": "Kept", "font_size": 12, "position": [2, 8]
    } });
    let composition: Composition = composition.into();
    let media = TestMedia::default();
    let submit = |renderer: &mut GpuCompositionRenderer, frame| {
        let bound = renderer
            .bind_frame(&composition, frame, &media, RenderTarget::Owned)
            .unwrap();
        renderer.upload_frame(&bound).unwrap();
        let raster = renderer.submit_frame(&bound).unwrap().raster;
        read_raster(renderer, raster)
    };
    for (discarded, submitted) in [(0, 1), (2, 3)] {
        renderer
            .bind_frame(&composition, discarded, &media, RenderTarget::Owned)
            .unwrap();
        let pixels = submit(&mut renderer, submitted);
        assert!(
            pixels == uncached(&mut reference, &composition, f64::from(submitted)),
            "frame {submitted} after discarding a bind of frame {discarded}"
        );
        assert_eq!(pixel(&pixels, 20, 2), [200, 30, 30, 255]);
    }
    assert!(renderer.skipped_pass_count() > 0);

    let older = renderer
        .bind_frame(&composition, 4, &media, RenderTarget::Owned)
        .unwrap();
    let current = renderer
        .bind_frame(&composition, 5, &media, RenderTarget::Owned)
        .unwrap();
    let stale = renderer.upload_frame(&older).unwrap_err();
    assert_eq!(stale.code(), "render.internal", "{stale}");
    renderer.upload_frame(&current).unwrap();
    let raster = renderer.submit_frame(&current).unwrap().raster;
    assert!(read_raster(&renderer, raster) == uncached(&mut reference, &composition, 5.0));
}

#[test]
fn kept_output_reaches_external_output_textures() {
    let gpu = gpu();
    let (mut renderer, mut reference) = (gpu.renderer(), gpu.renderer());
    let fully_static: Composition = canvas(vec![
        background(1, [200, 30, 30, 255]),
        square(2, json!([4, 4]), 8, [20, 40, 220, 255]),
        merge(3, &[1, 2]),
    ])
    .into();
    renderer.release_resources();
    at(&mut renderer, &fully_static, 0.0);
    let desc = renderer.output_desc().unwrap();
    let mut shown = Vec::new();
    for frame in 1..4 {
        let texture = renderer
            .gpu_renderer()
            .device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("external output"),
                size: desc.domain.storage_size.as_extent(),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: desc.format,
                usage: desc.usage | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
        let submitted = renderer
            .render(
                &fully_static,
                frame,
                &TestMedia::default(),
                RenderTarget::External(lumen_gpu::ExternalTexture::from_texture(texture)),
            )
            .unwrap();
        submitted.wait(&renderer.gpu_renderer().device).unwrap();
        let pixels = read_rgba8(
            renderer.gpu_renderer(),
            &submitted.raster.texture,
            desc.domain.storage_size,
        );
        assert!(
            pixels == uncached(&mut reference, &fully_static, f64::from(frame)),
            "external frame {frame}"
        );
        shown.push(pixels);
    }
    assert!(renderer.skipped_pass_count() > 0);
    assert_eq!(pixel(&shown[0], 20, 2), [200, 30, 30, 255]);
    assert_eq!(pixel(&shown[0], 8, 8), [20, 40, 220, 255]);
    assert!(shown[0] == shown[1] && shown[1] == shown[2]);
}

#[test]
fn kept_text_keeps_its_glyphs_in_the_shared_atlas() {
    let gpu = gpu();
    let (mut renderer, mut reference) = (gpu.renderer(), gpu.renderer());
    let text = |id: u32, content: &str, font_size: Value, x: u32| {
        json!({ "id": id, "type": "text", "params": {
            "content": content, "font_size": font_size, "scale": [0.03, 0.03],
            "position": [x, 0], "render_mode": "raster"
        } })
    };
    // Glyphs this large fill most of the atlas, so the static "W" and the "M" that grows on
    // frame 1 cannot both fit. A kept frame holds the "W" it already drew without placing it
    // again, so the "M" fits there too.
    let composition: Composition = scene(
        (256, 128),
        json!([
            text(1, "W", json!(3800), 0),
            text(2, "M", json!("=12 + 3788 * min(frame, 1)"), 120),
            merge(3, &[1, 2]),
        ]),
    )
    .frames(3)
    .into();
    let split = |pixels: &[u8]| -> (Vec<u8>, Vec<u8>) {
        let (mut left, mut right) = (Vec::new(), Vec::new());
        for row in pixels.as_chunks::<{ 256 * 4 }>().0 {
            let (start, end) = row.split_at(120 * 4);
            left.extend_from_slice(start);
            right.extend_from_slice(end);
        }
        (left, right)
    };
    for frame in 0..3 {
        let pixels = at(&mut renderer, &composition, f64::from(frame));
        assert!(nonblank(&pixels));
        let expected = uncached(&mut reference, &composition, f64::from(frame));
        if frame == 0 {
            assert!(
                pixels == expected,
                "frame 0 differs from an uncached render"
            );
        } else {
            let (left, right) = split(&pixels);
            assert!(left == split(&expected).0, "frame {frame} lost the W");
            assert!(nonblank(&right), "frame {frame} dropped the M");
        }
    }
    assert!(renderer.skipped_pass_count() > 0, "the static text is kept");
}

/// An 8 × 8 image from `source`, moved right by a transform when `transformed`, under a square
/// moving with the frame.
fn image_under_animated(source: &str, transformed: bool) -> Composition {
    let mut nodes = vec![json!({ "id": 1, "type": "media_in", "params": { "source": source } })];
    if transformed {
        nodes.push(json!({ "id": 2, "type": "transform", "in": { "source": 1 },
            "params": { "translate_x": 8 } }));
    }
    nodes.push(square(
        3,
        json!("=vec2(frame * 2, 10)"),
        4,
        [30, 220, 60, 255],
    ));
    nodes.push(merge(4, &[if transformed { 2 } else { 1 }, 3]));
    canvas(nodes).into()
}

fn image(source: &str, id: &str, color: [u8; 4]) -> TestMedia {
    TestMedia::default().image_with_id(source, id, 8, 8, color.repeat(64))
}

#[test]
fn a_static_image_is_kept_until_its_image_is_replaced() {
    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    let gpu = gpu();
    let (mut renderer, mut reference) = (gpu.renderer(), gpu.renderer());
    let (red, blue) = (image("logo", "logo#1", RED), image("logo", "logo#2", BLUE));
    for transformed in [true, false] {
        let composition = image_under_animated("logo", transformed);
        let x = if transformed { 12 } else { 4 };
        // Frame 3 replaces the image under the same source.
        for (frame, media, color, kept) in [
            (0, &red, RED, false),
            (1, &red, RED, true),
            (2, &red, RED, true),
            (3, &blue, BLUE, false),
            (4, &blue, BLUE, true),
        ] {
            let skipped = renderer.skipped_pass_count();
            let pixels = render(&mut renderer, &composition, f64::from(frame), media).rgba;
            reference.release_resources();
            let expected = render(&mut reference, &composition, f64::from(frame), media).rgba;
            assert!(
                pixels == expected,
                "frame {frame}, transformed {transformed}"
            );
            assert_eq!(pixel(&pixels, x, 4), color, "frame {frame}");
            // An image is brought into the working space by a pass, which is kept with it.
            let skips = renderer.skipped_pass_count() > skipped;
            assert_eq!(skips, kept, "frame {frame}");
        }
    }
}

#[test]
fn a_static_image_is_redrawn_when_replaced_by_one_of_another_size() {
    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    let gpu = gpu();
    let (mut renderer, mut reference) = (gpu.renderer(), gpu.renderer());
    let composition = image_under_animated("logo", true);
    let large = image("logo", "logo#1", RED);
    // A resized image is other bytes, so it has another identity.
    let small = TestMedia::default().image_with_id("logo", "logo#2", 2, 2, BLUE.repeat(4));
    for (frame, media, color) in [(0, &large, RED), (1, &large, RED), (2, &small, BLUE)] {
        let pixels = render(&mut renderer, &composition, f64::from(frame), media).rgba;
        reference.release_resources();
        let expected = render(&mut reference, &composition, f64::from(frame), media).rgba;
        assert!(pixels == expected, "frame {frame}");
        assert_eq!(pixel(&pixels, 9, 1), color, "frame {frame}");
        // Only the 8 × 8 image reaches this far.
        let far = if color == RED { RED } else { [0; 4] };
        assert_eq!(pixel(&pixels, 12, 4), far, "frame {frame}");
    }
}

#[test]
fn an_image_picked_by_frame_is_never_kept() {
    let gpu = gpu();
    let (mut renderer, mut reference) = (gpu.renderer(), gpu.renderer());
    let media = TestMedia::default()
        .image("red", 8, 8, [255, 0, 0, 255].repeat(64))
        .image("blue", 8, 8, [0, 0, 255, 255].repeat(64));
    let composition = image_under_animated("=if(frame % 2 < 1, 'red', 'blue')", true);
    for frame in 0..4 {
        let pixels = render(&mut renderer, &composition, f64::from(frame), &media).rgba;
        reference.release_resources();
        let expected = render(&mut reference, &composition, f64::from(frame), &media).rgba;
        assert!(pixels == expected, "frame {frame}");
        let color = if frame % 2 == 0 {
            [255, 0, 0, 255]
        } else {
            [0, 0, 255, 255]
        };
        assert_eq!(pixel(&pixels, 12, 4), color, "frame {frame}");
    }
    assert_eq!(renderer.skipped_pass_count(), 0);
}

/// `static_under_animated` with the static square placed by `position`.
fn static_square_at(position: Value) -> Composition {
    canvas(vec![
        background(1, [200, 30, 30, 255]),
        square(2, position, 8, [20, 40, 220, 255]),
        merge(3, &[1, 2]),
        square(4, json!("=vec2(frame * 2, 6)"), 4, [30, 220, 60, 255]),
        merge(5, &[3, 4]),
    ])
    .into()
}

#[test]
fn a_layer_whose_expressions_do_not_read_the_frame_is_kept() {
    let gpu = gpu();
    let (mut renderer, mut reference) = (gpu.renderer(), gpu.renderer());
    let mut literal = gpu.renderer();
    let composition = static_square_at(json!("=vec2(max(4, 2), 4)"));
    let kept = static_square_at(json!([4, 4]));
    for frame in 0..3 {
        let pixels = at(&mut renderer, &composition, f64::from(frame));
        assert!(pixels == uncached(&mut reference, &composition, f64::from(frame)));
        assert_eq!(pixel(&pixels, 8, 11), [20, 40, 220, 255], "frame {frame}");
        at(&mut literal, &kept, f64::from(frame));
        assert_eq!(
            renderer.recorded_pass_count(),
            literal.recorded_pass_count(),
            "frame {frame}"
        );
    }
}
