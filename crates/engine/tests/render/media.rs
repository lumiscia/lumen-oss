//! Images and video frames from a media store: their size, replacement, caching and release.

use std::sync::Arc;

use lumen_engine::gpu::GpuCompositionRenderer;
use lumen_engine::media::{CpuMediaFrame, MediaFrame};
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
        .compile_with_media(&composition, &media, wgpu::TextureFormat::Rgba8Unorm)
        .expect("frame 0 is only compiled ahead of time");

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
        &plate("plate", CANVAS, RED),
    );
    assert_eq!(large.pixel(CORNER.0, CORNER.1), RED);
    let small = render(
        &mut renderer,
        &composition,
        1.0,
        &plate("plate", (2, 2), BLUE),
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
    // The two images no plan binds fit exactly; the bound one doesn't count.
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
fn images_past_the_budget_are_freed_once_no_plan_binds_them() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    renderer.set_media_texture_cache_budget(0);
    let composition = media_in(json!({ "source": "plate" })).into();
    for (frame, color) in [RED, BLUE].into_iter().enumerate() {
        let media = plate(&format!("{color:?}"), CANVAS, color);
        render(&mut renderer, &composition, frame as f64, &media);
    }
    // Only the image the plan shows is kept.
    assert_eq!(renderer.cached_media_texture_count(), 1);
    renderer.set_static_output_reuse(false);
    assert_eq!(renderer.cached_media_texture_count(), 0);
}

#[test]
fn images_replaced_before_a_failed_upload_are_still_freed() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    renderer.set_media_texture_cache_budget(0);
    let composition = media_in(json!({ "source": "plate" })).into();
    render(&mut renderer, &composition, 0.0, &plate("red", CANVAS, RED));
    let media = plate("blue", CANVAS, BLUE);
    let mut bound = renderer.bind_frame_at(&composition, 1.0, &media).unwrap();
    // The valid image uploads, then this upload to a texture that doesn't exist fails.
    bound.use_media_texture(
        lumen_gpu::TextureId(u32::MAX),
        None,
        MediaFrame::CpuRgba(Arc::new(CpuMediaFrame {
            rgba: Arc::new(solid((1, 1), RED)),
            width: 1,
            height: 1,
            row_bytes: 4,
        })),
        lumen_gpu::Size::new(1, 1),
    );
    assert!(renderer.submit_bound_frame(&bound).is_err());
    assert_eq!(renderer.cached_media_texture_count(), 1);
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
    let output = renderer.compiled().unwrap().output;
    let gpu = renderer.gpu_renderer();
    let desc = gpu.texture_desc(output.texture).unwrap();
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
            lumen_gpu::ExternalTexture::from_texture(texture, desc.domain),
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
