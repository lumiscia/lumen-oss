//! Output kept between frames: what static layers keep, and how it survives plan switches,
//! edits, external targets and the shared text atlas.

use lumen_engine::{composition::Composition, gpu::GpuCompositionRenderer, node::NodeId};
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
    renderer.set_frame_graph(false);
    renderer.release_resources();
    at(renderer, composition, frame)
}

fn read_raster(
    renderer: &GpuCompositionRenderer,
    raster: lumen_engine::gpu::RasterHandle,
) -> Vec<u8> {
    let gpu = renderer.gpu_renderer();
    read_rgba8(
        gpu,
        gpu.texture(raster.texture).unwrap(),
        raster.domain.storage_size,
    )
}

fn nonblank(pixels: &[u8]) -> bool {
    pixels.iter().any(|&byte| byte > 0)
}

#[test]
fn zero_plan_cache_budget_keeps_the_plan_a_bound_frame_renders_with() {
    let gpu = gpu();
    let (mut renderer, mut reference) = (gpu.renderer(), gpu.renderer());
    let media = TestMedia::default();
    let composition: Composition = static_under_animated().into();
    assert!(nonblank(&at(&mut renderer, &composition, 0.0)));
    let bound = renderer.bind_frame_at(&composition, 1.0, &media).unwrap();
    renderer.set_plan_cache_budget(0);
    let (raster, _) = renderer.submit_bound_frame(&bound).unwrap();
    assert!(read_raster(&renderer, raster) == uncached(&mut reference, &composition, 1.0));
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
            cached.push(renderer.cached_pass_count());
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
            "one plan keeps the same passes every frame"
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

#[test]
fn kept_output_survives_plan_switches_and_is_dropped_with_evicted_plans() {
    let gpu = gpu();
    let mut reference = gpu.renderer();
    let composition = switching_static_layers();
    // Each of the three selections is its own plan; frames 6 and 7 select the first again.
    for budget in [None, Some(0)] {
        let mut renderer = gpu.renderer();
        if let Some(budget) = budget {
            renderer.set_plan_cache_budget(budget);
        }
        let mut compiled_before_revisit = 0;
        for frame in 0..8 {
            if frame == 6 {
                compiled_before_revisit = renderer.plan_compile_count();
            }
            let pixels = at(&mut renderer, &composition, f64::from(frame));
            let expected = uncached(&mut reference, &composition, f64::from(frame));
            assert!(pixels == expected, "frame {frame} with budget {budget:?}");
            assert!(nonblank(&pixels));
        }
        assert!(renderer.cached_pass_count() > 0, "budget {budget:?}");
        // A recorded frame has no plans to keep or evict; a release is what drops its kept outputs.
        if renderer.frame_graph_enabled() {
            assert!(renderer.frame_graph_stats().unwrap().retained_bytes > 0);
            renderer.release_resources();
            assert_eq!(renderer.frame_graph_stats().unwrap().retained_bytes, 0);
            continue;
        }
        assert_eq!(
            renderer.plan_compile_count() > compiled_before_revisit,
            budget.is_some(),
            "revisiting the first selection with budget {budget:?}"
        );
        let kept = renderer.cached_plan_count();
        assert!(
            if budget.is_some() {
                kept <= 2
            } else {
                kept == 3
            },
            "{kept} plans kept with budget {budget:?}"
        );
    }
}

#[test]
fn replacing_a_kept_texture_renders_it_again() {
    let gpu = gpu();
    let (mut renderer, mut reference) = (gpu.renderer(), gpu.renderer());
    let composition: Composition = static_under_animated().into();
    for frame in 0..2 {
        at(&mut renderer, &composition, f64::from(frame));
    }
    assert!(renderer.cached_pass_count() > 0, "the static layer is kept");
    // A recorded frame's kept textures are the recorder's own, with no id to replace them through.
    if renderer.frame_graph_enabled() {
        let pixels = at(&mut renderer, &composition, 2.0);
        assert!(pixels == uncached(&mut reference, &composition, 2.0));
        return;
    }
    let kept = renderer
        .compiled()
        .unwrap()
        .node_output(NodeId::new(3), &[])
        .unwrap();
    let gpu = renderer.gpu_renderer_mut();
    let desc = gpu.texture_desc(kept.texture).unwrap();
    let blank = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("blank"),
        size: desc.domain.storage_size.as_extent(),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: desc.format,
        usage: desc.usage,
        view_formats: &[],
    });
    gpu.replace_texture(kept.texture, blank, desc).unwrap();
    let pixels = at(&mut renderer, &composition, 2.0);
    let expected = uncached(&mut reference, &composition, 2.0);
    assert!(nonblank(&expected));
    assert!(pixels == expected, "the replaced texture is rendered again");
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
        let bound = renderer.bind_frame(&composition, frame, &media).unwrap();
        let (raster, _) = renderer.submit_bound_frame(&bound).unwrap();
        read_raster(renderer, raster)
    };
    for (discarded, submitted) in [(0, 1), (2, 3)] {
        renderer
            .bind_frame(&composition, discarded, &media)
            .unwrap();
        let pixels = submit(&mut renderer, submitted);
        assert!(
            pixels == uncached(&mut reference, &composition, f64::from(submitted)),
            "frame {submitted} after discarding a bind of frame {discarded}"
        );
        assert!(nonblank(&pixels));
    }
    assert!(renderer.cached_pass_count() > 0);

    let older = renderer.bind_frame(&composition, 4, &media).unwrap();
    let current = renderer.bind_frame(&composition, 5, &media).unwrap();
    assert!(renderer.upload_bound_frame(&older).is_err());
    renderer.upload_bound_frame(&current).unwrap();
    let (raster, _) = renderer.submit_render().unwrap();
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
            .render_frame_into_external(
                &fully_static,
                frame,
                &TestMedia::default(),
                lumen_gpu::ExternalTexture::from_texture(texture, desc.domain),
            )
            .unwrap();
        submitted.wait(&renderer.gpu_renderer().device).unwrap();
        let pixels = read_rgba8(
            renderer.gpu_renderer(),
            submitted.texture(),
            desc.domain.storage_size,
        );
        assert!(
            pixels == uncached(&mut reference, &fully_static, f64::from(frame)),
            "external frame {frame}"
        );
        shown.push(pixels);
    }
    assert!(renderer.cached_pass_count() > 0);
    assert!(nonblank(&shown[0]));
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
    // frame 1 cannot both fit; the "W" is drawn first, so it keeps its glyph. A recorded frame
    // keeps the "W" it already drew without placing it again, so the "M" fits there too.
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
    let recording = renderer.frame_graph_enabled();
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
        if recording && frame > 0 {
            let (left, right) = split(&pixels);
            assert!(left == split(&expected).0, "frame {frame} lost the W");
            assert!(nonblank(&right), "frame {frame} dropped the M");
        } else {
            assert!(
                pixels == expected,
                "frame {frame} differs from an uncached render"
            );
        }
    }
    assert!(renderer.cached_pass_count() > 0, "the static text is kept");
}
