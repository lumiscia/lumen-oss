//! Frames recorded rather than bound to plans, whichever path the suite runs on: what fails before
//! the first frame, and who owns a frame rendered into a texture of the caller's.

use lumen_engine::{composition::Composition, gpu::GpuCompositionRenderer};
use lumen_gpu::{ExternalTexture, wgpu};
use serde_json::{Value, json};

use crate::support::{Scene, TestMedia, gpu, read_rgba8, render, scene};

const CANVAS: (u32, u32) = (64, 32);

fn recording(renderer: &mut GpuCompositionRenderer) {
    renderer.set_frame_graph(true);
}

/// Everything a frame may record once, some of it only from a later frame on: the feather and blur
/// sweep across their levels, the text starts moving, and the clip comes into view.
fn every_program() -> Scene {
    Scene(json!({
        "timeline": { "fps": 30, "duration_frames": 60 },
        "render_settings": { "width": CANVAS.0, "height": CANVAS.1 },
        "compositions": {
            "inner": {
                "timeline": { "duration_frames": 60 },
                "nodes": [
                    { "id": 1, "type": "background", "params": { "paint": [40, 40, 200, 255] } },
                    { "id": 2, "type": "wgsl_shader", "params": {} },
                    { "id": 3, "type": "media_output" }
                ],
                "connections": [
                    { "from_node": 1, "to_node": 2, "to_port": "source" },
                    { "from_node": 2, "to_node": 3, "to_port": "source" }
                ]
            }
        },
        "nodes": [
            { "id": 1, "type": "background", "params": { "paint": [200, 30, 30, 255] } },
            { "id": 2, "type": "shape", "params": {
                "position": "=vec2(frame, 4)", "width": 12, "height": 12, "fill_paint": "#ffffff"
            } },
            { "id": 3, "type": "path", "params": {
                "data": "M 2 2 L 20 2 L 10 16 Z", "fill_paint": "#30dc40"
            } },
            { "id": 4, "type": "text", "params": {
                "content": "text", "font_size": 12, "position": "=vec2(frame, 8)",
                "opacity": "=1 - frame / 60", "motion_blur": 180
            } },
            { "id": 5, "type": "clip", "params": { "composition": "inner", "start": 30 } },
            { "id": 6, "type": "exposure", "params": { "exposure": 0.2 } },
            { "id": 7, "type": "hue_saturation", "params": { "saturation": 0.8 } },
            { "id": 8, "type": "levels", "params": { "gamma": 1.1 } },
            { "id": 9, "type": "opacity", "params": { "opacity": 0.9 } },
            { "id": 10, "type": "transform", "params": { "rotate": "=frame" } },
            { "id": 11, "type": "crop", "params": { "width": 40, "height": 20 } },
            { "id": 12, "type": "resize", "params": { "width": 32, "height": 16 } },
            { "id": 13, "type": "shadow", "params": { "radius": "=frame * 2", "offset_x": 2 } },
            { "id": 14, "type": "blur", "params": { "radius": "=frame * 2" } },
            { "id": 15, "type": "merge", "params": { "feather": "=frame" } },
            { "id": 16, "type": "boolean", "params": {} },
            { "id": 17, "type": "switch", "params": { "selected_layer": "=min(frame, 1)" } },
            { "id": 18, "type": "raster_multimerge" },
            { "id": 19, "type": "media_output" }
        ],
        "connections": [
            { "from_node": 2, "to_node": 6, "to_port": "source" },
            { "from_node": 6, "to_node": 7, "to_port": "source" },
            { "from_node": 7, "to_node": 8, "to_port": "source" },
            { "from_node": 8, "to_node": 9, "to_port": "source" },
            { "from_node": 9, "to_node": 10, "to_port": "source" },
            { "from_node": 10, "to_node": 11, "to_port": "source" },
            { "from_node": 11, "to_node": 12, "to_port": "source" },
            { "from_node": 3, "to_node": 13, "to_port": "source" },
            { "from_node": 13, "to_node": 14, "to_port": "source" },
            { "from_node": 1, "to_node": 15, "to_port": "base" },
            { "from_node": 14, "to_node": 15, "to_port": "overlay" },
            { "from_node": 2, "to_node": 15, "to_port": "mask" },
            { "from_node": 1, "to_node": 16, "to_port": "a" },
            { "from_node": 15, "to_node": 16, "to_port": "b" },
            { "from_node": 12, "to_node": 17, "to_port": "layers" },
            { "from_node": 4, "to_node": 17, "to_port": "layers" },
            { "from_node": 15, "to_node": 18, "to_port": "layers" },
            { "from_node": 17, "to_node": 18, "to_port": "layers" },
            { "from_node": 5, "to_node": 18, "to_port": "layers" },
            { "from_node": 16, "to_node": 18, "to_port": "layers" },
            { "from_node": 18, "to_node": 19, "to_port": "source" }
        ]
    }))
}

fn compile(renderer: &mut GpuCompositionRenderer, composition: &Composition) -> Result<(), String> {
    renderer
        .compile_with_media(
            composition,
            &TestMedia::default(),
            wgpu::TextureFormat::Rgba8Unorm,
        )
        .map_err(|error| error.to_string())
}

#[test]
fn compiling_builds_every_program_a_frame_records_with() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    recording(&mut renderer);
    let composition = every_program().into();
    compile(&mut renderer, &composition).unwrap();
    let programs = |renderer: &GpuCompositionRenderer| renderer.frame_graph_stats().unwrap();
    let built = programs(&renderer).recorder.programs_created;
    assert!(built > 0);
    for frame in [0.0, 1.0, 5.0, 20.0, 31.0, 59.0] {
        render(&mut renderer, &composition, frame, &TestMedia::default());
        assert_eq!(
            programs(&renderer).recorder.programs_created,
            built,
            "frame {frame} built a program"
        );
    }
}

#[test]
fn a_shader_that_does_not_compile_fails_before_the_first_frame() {
    let mut composition = every_program();
    composition.0["compositions"]["inner"]["nodes"][1]["params"]["shader"] = json!(
        "@compute @workgroup_size(8, 8, 1)\n\
         fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) { let x: NotAType = 1.0; }"
    );
    let composition = composition.into();
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    recording(&mut renderer);
    let error = compile(&mut renderer, &composition).unwrap_err();
    assert!(error.contains("NotAType"), "{error}");
}

fn target(renderer: &GpuCompositionRenderer, like: lumen_gpu::TextureDesc) -> wgpu::Texture {
    renderer
        .gpu_renderer()
        .device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("held target"),
            size: like.domain.storage_size.as_extent(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: like.format,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
}

fn animated() -> Composition {
    let nodes: Value = json!([
        { "id": 1, "type": "background", "params": {
            "paint": [20, 20, 60, 255], "width": CANVAS.0, "height": CANVAS.1
        } },
        { "id": 2, "type": "shape", "params": {
            "position": "=vec2(frame * 3, 8)", "width": 10, "height": 10, "fill_paint": "#ffffff"
        } },
        { "id": 3, "type": "blur", "params": { "radius": 2 }, "in": { "source": 2 } },
        { "id": 4, "type": "merge", "in": { "base": 1, "overlay": 3 } }
    ]);
    scene(CANVAS, nodes).frames(12).into()
}

/// A target the caller holds doesn't change once its frame is submitted, while later frames turn
/// the pool over and destroy what it evicts.
#[test]
fn a_held_external_target_keeps_its_frame_while_the_pool_turns_over() {
    let gpu = gpu();
    let (mut renderer, mut reference) = (gpu.renderer(), gpu.renderer());
    recording(&mut renderer);
    renderer.set_frame_graph_pool_budget(0);
    recording(&mut reference);
    let composition = animated();
    let media = TestMedia::default();
    render(&mut reference, &composition, 0.0, &media);
    let desc = reference.output_desc().unwrap();
    let size = desc.domain.storage_size;
    let mut held = Vec::new();
    for frame in 0..8u32 {
        let texture = target(&renderer, desc);
        held.push(
            renderer
                .render_frame_into_external(
                    &composition,
                    frame,
                    &media,
                    ExternalTexture::from_texture(texture, desc.domain),
                )
                .unwrap(),
        );
    }
    let stats = renderer.frame_graph_stats().unwrap();
    assert_eq!(
        stats.recorder.external_copies, 0,
        "the passes wrote directly"
    );
    assert!(stats.recorder.textures_destroyed > 0, "the pool evicted");
    for (frame, submitted) in held.iter().enumerate() {
        submitted.wait(&renderer.gpu_renderer().device).unwrap();
        let shown = read_rgba8(renderer.gpu_renderer(), submitted.texture(), size);
        let expected = render(&mut reference, &composition, frame as f64, &media).rgba;
        assert!(shown == expected, "frame {frame} changed while it was held");
    }
    drop(held);
    render(&mut renderer, &composition, 8.0, &media);
    render(&mut renderer, &composition, 9.0, &media);
    let target_bytes = u64::from(size.width) * u64::from(size.height) * 4;
    assert!(
        renderer.frame_graph_stats().unwrap().adopted_bytes < target_bytes,
        "the graph lets go of targets once only it holds them"
    );
}

/// A ring of targets, as an encoder cycles them: a target comes back for a later frame.
#[test]
fn a_target_used_again_shows_the_frame_rendered_into_it_last() {
    let gpu = gpu();
    let (mut renderer, mut reference) = (gpu.renderer(), gpu.renderer());
    recording(&mut renderer);
    renderer.set_frame_graph_pool_budget(0);
    recording(&mut reference);
    let composition = animated();
    let media = TestMedia::default();
    render(&mut reference, &composition, 0.0, &media);
    let desc = reference.output_desc().unwrap();
    let ring: Vec<_> = (0..3)
        .map(|_| std::sync::Arc::new(target(&renderer, desc)))
        .collect();
    for frame in 0..9u32 {
        let submitted = renderer
            .render_frame_into_external(
                &composition,
                frame,
                &media,
                ExternalTexture::new(ring[frame as usize % 3].clone(), desc.domain),
            )
            .unwrap();
        submitted.wait(&renderer.gpu_renderer().device).unwrap();
        let shown = read_rgba8(
            renderer.gpu_renderer(),
            submitted.texture(),
            desc.domain.storage_size,
        );
        let expected = render(&mut reference, &composition, f64::from(frame), &media).rgba;
        assert!(shown == expected, "frame {frame} differs");
    }
}

/// A static output stays in the pool for later frames, so a target gets a copy of it.
#[test]
fn a_kept_output_is_copied_to_the_target_and_stays_kept() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    recording(&mut renderer);
    let nodes = json!([
        { "id": 1, "type": "background", "params": {
            "paint": [200, 30, 30, 255], "width": CANVAS.0, "height": CANVAS.1
        } },
        { "id": 2, "type": "shape", "params": {
            "position": [4, 4], "width": 8, "height": 8, "fill_paint": "#2040dc"
        } },
        { "id": 3, "type": "merge", "in": { "base": 1, "overlay": 2 } }
    ]);
    let composition: Composition = scene(CANVAS, nodes).frames(4).into();
    let media = TestMedia::default();
    let expected = render(&mut renderer, &composition, 0.0, &media).rgba;
    let desc = renderer.output_desc().unwrap();
    for frame in 1..4 {
        let submitted = renderer
            .render_frame_into_external(
                &composition,
                frame,
                &media,
                ExternalTexture::from_texture(target(&renderer, desc), desc.domain),
            )
            .unwrap();
        submitted.wait(&renderer.gpu_renderer().device).unwrap();
        let shown = read_rgba8(
            renderer.gpu_renderer(),
            submitted.texture(),
            desc.domain.storage_size,
        );
        assert!(shown == expected, "frame {frame} differs");
    }
    let stats = renderer.frame_graph_stats().unwrap();
    assert_eq!(stats.recorder.external_copies, 3);
    assert_eq!(
        stats.recorder.passes,
        3 + 3,
        "the first frame's passes alone"
    );
}
