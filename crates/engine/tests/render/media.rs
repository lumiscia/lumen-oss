//! Images and video frames from a media store: their size, replacement, caching and release.

use lumen_engine::error::MediaReason;
use lumen_engine::gpu::GpuCompositionRenderer;
use lumen_engine::media::{AssetKind, MediaLocation, NodeParamAt};
use lumen_engine::node::NodeId;
use lumen_gpu::wgpu;
use serde_json::{Value, json};

use crate::support::{Case, Expect, Scene, TestMedia, check, gpu, read_rgba8, render, scene};

const CANVAS: (u32, u32) = (32, 16);
const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];
const CLEAR: [u8; 4] = [0; 4];

/// The last pixel of the canvas, which only a canvas-sized image covers.
const CORNER: (u32, u32) = (CANVAS.0 - 1, CANVAS.1 - 1);

fn solid((width, height): (u32, u32), color: [u8; 4]) -> Vec<u8> {
    color.repeat(width as usize * height as usize)
}

fn media_in(params: Value) -> Scene {
    scene(
        CANVAS,
        json!([{ "id": 1, "type": "media_in", "params": params }]),
    )
    .frames(12)
}

/// An image of `source` that the engine tells from another by `id`.
fn plate(id: &str, size: (u32, u32), color: [u8; 4]) -> TestMedia {
    TestMedia::default().image_with_id("plate", id, size.0, size.1, solid(size, color))
}

#[test]
fn a_missing_source_only_fails_the_frames_that_reach_it() {
    let gpu = gpu();
    let composition: lumen_engine::composition::Composition = scene(
        CANVAS,
        json!([
            { "id": 1, "type": "media_in", "params": { "kind": "image", "source": "gone" } },
            { "id": 2, "type": "background", "params": { "paint": "#00ff00" } },
            { "id": 3, "type": "switch", "params": { "selected_layer": "=if(frame < 5, 0, 1)" },
              "in": { "layers": [1, 2] } },
        ]),
    )
    .frames(12)
    .into();
    let media = TestMedia::default();
    let mut renderer = gpu.renderer();
    renderer
        .compile_with_output_format(&composition, wgpu::TextureFormat::Rgba8Unorm)
        .expect("a missing source only fails a frame that reaches it");

    assert_eq!(
        render(&mut renderer, &composition, 10.0, &media).pixel(0, 0),
        GREEN
    );
    let error = renderer
        .render_frame_at(&composition, 0.0, &media)
        .unwrap_err();
    assert!(error.to_string().contains("gone"), "{error}");
}

#[test]
fn a_missing_source_reports_where_it_is_used() {
    let gpu = gpu();
    let composition: lumen_engine::composition::Composition =
        media_in(json!({ "kind": "image", "source": "gone.png" })).into();
    let error = gpu
        .renderer()
        .render_frame_at(&composition, 0.0, &TestMedia::default())
        .unwrap_err();
    let failure = error.media_failure().expect("a media failure");
    assert_eq!(failure.code(), "media.not_found");
    assert_eq!(failure.source, "gone.png");
    assert_eq!(failure.wanted, [AssetKind::Image]);
    assert_eq!(
        failure.at,
        Some(MediaLocation::Node(NodeParamAt {
            template: None,
            node: NodeId::new(1),
            param: "source".to_string(),
        }))
    );
}

#[test]
fn an_image_wider_than_the_device_allows_is_too_large() {
    let gpu = gpu();
    let composition: lumen_engine::composition::Composition =
        media_in(json!({ "kind": "image", "source": "wide.png" })).into();
    let mut renderer = gpu.renderer();
    let limit = u64::from(
        renderer
            .gpu_renderer()
            .device
            .limits()
            .max_texture_dimension_2d,
    );
    let width = limit as u32 + 1;
    let media = TestMedia::default().image("wide.png", width, 1, solid((width, 1), RED));
    let error = renderer
        .render_frame_at(&composition, 0.0, &media)
        .unwrap_err();
    let failure = error.media_failure().expect("a media failure");
    assert!(
        matches!(
            failure.reason,
            MediaReason::TooLarge { limit: found, actual: Some(actual), .. }
                if found == limit && actual == u64::from(width)
        ),
        "{error}"
    );
    assert_eq!(failure.source, "wide.png");
    assert!(failure.at.is_some(), "{error}");
}

#[test]
fn media_of_another_size_renders_at_its_own_size() {
    check(vec![
        Case::new(
            "a source expression picks an image of another size",
            media_in(json!({ "source": "=if(frame < 1, 'large', 'small')" })),
            Expect::PerFrame(vec![
                Expect::Probes(vec![(CORNER.0, CORNER.1, RED)]),
                Expect::Probes(vec![(1, 1, BLUE), (CORNER.0, CORNER.1, CLEAR)]),
            ]),
        )
        .media(
            TestMedia::default()
                .image("large", CANVAS.0, CANVAS.1, solid(CANVAS, RED))
                .image("small", 2, 2, solid((2, 2), BLUE)),
        )
        .frames([0.0, 1.0])
        .tolerance(0),
    ]);
}

#[test]
fn an_image_replaced_by_one_of_another_size_renders_at_the_new_size() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let composition = media_in(json!({ "source": "plate" })).into();
    let large = render(
        &mut renderer,
        &composition,
        0.0,
        &plate("plate#large", CANVAS, RED),
    );
    assert_eq!(large.pixel(CORNER.0, CORNER.1), RED);
    let small = render(
        &mut renderer,
        &composition,
        1.0,
        &plate("plate#small", (2, 2), BLUE),
    );
    assert_eq!(small.pixel(1, 1), BLUE);
    assert_eq!(small.pixel(CORNER.0, CORNER.1), CLEAR);
}

#[test]
fn replacing_an_image_with_one_of_the_same_size_shows_its_pixels() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let composition = media_in(json!({ "source": "plate" })).into();
    for (frame, (id, color)) in [("plate#1", RED), ("plate#2", BLUE), ("plate#3", RED)]
        .into_iter()
        .enumerate()
    {
        let shown = render(
            &mut renderer,
            &composition,
            frame as f64,
            &plate(id, CANVAS, color),
        );
        assert_eq!(shown.pixel(1, 1), color, "{id}");
    }
}

#[test]
fn unbound_images_within_the_budget_stay_cached() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let colors = [RED, GREEN, BLUE];
    // The two images the frame doesn't show fit exactly; the shown one doesn't count.
    renderer.set_media_texture_cache_budget(2 * u64::from(CANVAS.0 * CANVAS.1 * 4));
    let composition = media_in(json!({ "source": "plate" })).into();
    for frame in 0..9 {
        let color = colors[frame % 3];
        let media = plate(&format!("{color:?}"), CANVAS, color);
        let shown = render(&mut renderer, &composition, frame as f64, &media);
        assert_eq!(shown.pixel(1, 1), color, "frame {frame}");
    }
    assert_eq!(renderer.media_texture_upload_count(), 3);
    assert_eq!(renderer.cached_media_texture_count(), 3);
}

#[test]
fn images_past_the_budget_are_freed_once_no_frame_shows_them() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    renderer.set_media_texture_cache_budget(0);
    let composition = media_in(json!({ "source": "plate" })).into();
    for (frame, color) in [RED, BLUE].into_iter().enumerate() {
        let media = plate(&format!("{color:?}"), CANVAS, color);
        render(&mut renderer, &composition, frame as f64, &media);
    }
    // Only the image the frame shows is kept.
    assert_eq!(renderer.cached_media_texture_count(), 1);
    renderer.set_static_output_reuse(false);
    assert_eq!(renderer.cached_media_texture_count(), 0);
}

#[test]
fn images_a_failed_recording_bound_are_freed() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    renderer.set_media_texture_cache_budget(0);
    let composition = scene(
        CANVAS,
        json!([
            { "id": 1, "type": "media_in", "params": { "kind": "image", "source": "plate" } },
            { "id": 2, "type": "opacity", "in": { "source": 1 }, "params": { "opacity": "=word" } },
        ]),
    )
    .frames(12)
    .into();
    for attempt in 0..5 {
        let media = plate(&format!("plate:{attempt}"), CANVAS, RED);
        assert!(renderer.bind_frame(&composition, attempt, &media).is_err());
        assert_eq!(renderer.cached_media_texture_count(), 0, "{attempt}");
        assert_eq!(renderer.frame_graph_stats().adopted_bytes, 0, "{attempt}");
    }
}

#[test]
fn video_frames_do_not_overwrite_cached_images() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let media = TestMedia::default()
        .image("a", CANVAS.0, CANVAS.1, solid(CANVAS, RED))
        .image("b", CANVAS.0, CANVAS.1, solid(CANVAS, GREEN))
        .image("c", CANVAS.0, CANVAS.1, solid(CANVAS, YELLOW))
        .still_video("clip", CANVAS.0, CANVAS.1, solid(CANVAS, BLUE));
    let composition = media_in(json!({
        "kind": "=if(frame == 1, 1, 0)",
        "source": "=if(frame == 1, 'clip', if(frame == 2, 'b', if(frame == 3, 'c', 'a')))"
    }))
    .into();
    for (frame, expected) in [RED, BLUE, GREEN, YELLOW, RED].into_iter().enumerate() {
        let shown = render(&mut renderer, &composition, frame as f64, &media);
        assert_eq!(shown.pixel(1, 1), expected, "frame {frame}");
    }
}

/// Renders `composition` into a texture of its own and reads it back.
fn render_external(
    renderer: &mut GpuCompositionRenderer,
    composition: &lumen_engine::composition::Composition,
    media: &TestMedia,
) -> Vec<u8> {
    let desc = renderer.output_desc().unwrap();
    let gpu = renderer.gpu_renderer();
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
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
            composition,
            0,
            media,
            lumen_gpu::ExternalTexture::from_texture(texture),
        )
        .unwrap();
    submitted.wait(&renderer.gpu_renderer().device).unwrap();
    read_rgba8(
        renderer.gpu_renderer(),
        submitted.texture(),
        desc.domain.storage_size,
    )
}

#[test]
fn canvas_sized_media_passthrough_writes_external_targets() {
    let gpu = gpu();
    let color = [40, 120, 200, 255];
    let media = TestMedia::default()
        .image("plate", CANVAS.0, CANVAS.1, solid(CANVAS, color))
        .still_video("plate", CANVAS.0, CANVAS.1, solid(CANVAS, color));
    for kind in ["image", "video"] {
        let composition = media_in(json!({ "kind": kind, "source": "plate" })).into();
        let mut renderer = gpu.renderer();
        render(&mut renderer, &composition, 0.0, &media);
        let pixels = render_external(&mut renderer, &composition, &media);
        assert!(
            pixels == solid(CANVAS, color),
            "media kind {kind} left its external target at {:?}",
            &pixels[..4]
        );
    }
}
