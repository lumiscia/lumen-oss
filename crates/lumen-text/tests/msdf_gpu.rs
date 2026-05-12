#![cfg(feature = "experimental-msdf")]

use std::sync::mpsc;

use lumen_gpu::{
    BindGroupLayoutSpec, Binding, BindingLayoutEntry, BufferDesc, ComputePassDesc,
    ComputeProgramDesc, Dispatch, Draw, DrawCommand, FrameUpdate, LoadOp, ProgramDesc,
    RenderPassDesc, RenderPlan, RenderProgramDesc, RenderTargetRef, Renderer, Size, TextureDesc,
    TextureId, wgpu,
};
use lumen_text::{
    AtlasConfig, GpuMsdfGlobals, GpuTextGlobals, MSDF_GENERATOR_SHADER, MSDF_TEXT_SHADER,
    TextLayoutRequest, TextSystem,
};

#[test]
fn large_msdf_glyph_generates_non_solid_distance_field() {
    let scenario = MsdfScenario {
        text: "B",
        font_size: 240.0,
        origin: [0.0, 240.0],
        px_range: 8,
        atlas_size: Size::new(512, 512),
        target_size: Size::new(512, 512),
        max_glyphs: 1,
        max_segments: 4096,
    };
    let Some(result) = render_msdf_scenario(scenario) else {
        return;
    };

    assert_eq!(result.atlas.jobs.len(), 1);

    let bytes =
        read_texture_rgba16_float(&result.renderer, result.atlas_texture, result.atlas_size);
    let job = result.atlas.jobs[0];
    let mut outside = 0usize;
    let mut inside = 0usize;
    for y in job.atlas_rect[1]..job.atlas_rect[1] + job.atlas_rect[3] {
        for x in job.atlas_rect[0]..job.atlas_rect[0] + job.atlas_rect[2] {
            let index = ((y * result.atlas_size.width + x) * 4) as usize;
            let median = median3_f16(bytes[index], bytes[index + 1], bytes[index + 2]);
            if median < 0.44 {
                outside += 1;
            } else if median > 0.56 {
                inside += 1;
            }
        }
    }

    assert!(outside > 0, "large glyph MSDF has no outside pixels");
    assert!(inside > 0, "large glyph MSDF has no inside pixels");

    let (opaque, total) = rendered_alpha_stats(
        &result.renderer,
        result.output,
        result.target_size,
        result.coverage_rect(),
    );
    assert!(
        opaque < total * 9 / 10,
        "large glyph rendered as a mostly solid block: opaque={opaque} total={total}",
    );
}

#[test]
fn large_multiline_msdf_text_does_not_render_as_blocks() {
    let scenario = MsdfScenario {
        text: "Large text\nat 150 pt\non HD",
        font_size: 150.0,
        origin: [120.0, 180.0],
        px_range: 16,
        atlas_size: Size::new(2048, 2048),
        target_size: Size::new(1920, 1080),
        max_glyphs: 64,
        max_segments: 32768,
    };
    let Some(result) = render_msdf_scenario(scenario) else {
        return;
    };

    assert!(
        result.atlas.jobs.len() >= 8,
        "expected several outline glyphs to use MSDF jobs, got {}",
        result.atlas.jobs.len()
    );
    assert_eq!(
        result.atlas.glyph_count as usize,
        result.atlas.instances.len()
    );

    if let Ok(path) = std::env::var("LUMEN_TEXT_MSDF_GPU_SNAPSHOT") {
        write_texture_png(&result.renderer, result.output, result.target_size, path);
    }

    let (opaque, total) = rendered_alpha_stats(
        &result.renderer,
        result.output,
        result.target_size,
        result.coverage_rect(),
    );
    assert!(
        opaque > 0,
        "large multiline text produced no opaque pixels in its coverage rect",
    );
    assert!(
        opaque < total * 7 / 10,
        "large multiline text rendered as mostly solid blocks: opaque={opaque} total={total}",
    );
}

struct MsdfScenario<'a> {
    text: &'a str,
    font_size: f32,
    origin: [f32; 2],
    px_range: u32,
    atlas_size: Size,
    target_size: Size,
    max_glyphs: usize,
    max_segments: usize,
}

struct MsdfRenderResult {
    renderer: Renderer,
    atlas: lumen_text::GpuHybridAtlasRender,
    atlas_texture: TextureId,
    output: TextureId,
    atlas_size: Size,
    target_size: Size,
}

impl MsdfRenderResult {
    fn coverage_rect(&self) -> [u32; 4] {
        let mut x0 = self.target_size.width;
        let mut y0 = self.target_size.height;
        let mut x1 = 0;
        let mut y1 = 0;
        for instance in &self.atlas.instances {
            x0 = x0.min(instance.rect[0].max(0.0) as u32);
            y0 = y0.min(instance.rect[1].max(0.0) as u32);
            x1 = x1.min(self.target_size.width).max(
                (instance.rect[0] + instance.rect[2]).clamp(0.0, self.target_size.width as f32)
                    as u32,
            );
            y1 = y1.min(self.target_size.height).max(
                (instance.rect[1] + instance.rect[3]).clamp(0.0, self.target_size.height as f32)
                    as u32,
            );
        }
        [x0, y0, x1, y1]
    }
}

fn render_msdf_scenario(scenario: MsdfScenario<'_>) -> Option<MsdfRenderResult> {
    let Some(mut renderer) = renderer() else {
        return None;
    };
    let mut text_system = TextSystem::new();
    let mut request = TextLayoutRequest::new(scenario.text);
    request.font_size = scenario.font_size;
    request.origin = scenario.origin;
    let layout = text_system.layout(&request);
    let atlas = text_system.render_gpu_hybrid_atlas(
        &layout,
        AtlasConfig {
            width: scenario.atlas_size.width,
            height: scenario.atlas_size.height,
            px_range: scenario.px_range,
        },
        scenario.max_glyphs,
        scenario.max_segments,
        scenario.atlas_size.width * scenario.atlas_size.height,
    );

    let mut builder = RenderPlan::builder();
    let atlas_texture = builder.texture(
        Some("large msdf atlas".to_string()),
        TextureDesc::storage(scenario.atlas_size, wgpu::TextureFormat::Rgba16Float),
    );
    let globals_buffer = builder.buffer(
        Some("large msdf globals".to_string()),
        BufferDesc::uniform(std::mem::size_of::<GpuMsdfGlobals>() as u64),
    );
    let text_globals_buffer = builder.buffer(
        Some("large msdf text globals".to_string()),
        BufferDesc::uniform(std::mem::size_of::<GpuTextGlobals>() as u64),
    );
    let instances_size =
        (atlas.instances.len().max(1) * std::mem::size_of::<lumen_text::GpuGlyphInstance>()) as u64;
    let jobs_size =
        (atlas.jobs.len().max(1) * std::mem::size_of::<lumen_text::GpuMsdfJob>()) as u64;
    let segments_size =
        (atlas.segments.len().max(1) * std::mem::size_of::<lumen_text::GpuMsdfSegment>()) as u64;
    let pixel_jobs_size = (atlas.pixel_jobs.len().max(1) * std::mem::size_of::<u32>()) as u64;
    let instances_buffer = builder.buffer(
        Some("large msdf instances".to_string()),
        BufferDesc::storage(instances_size),
    );
    let jobs_buffer = builder.buffer(
        Some("large msdf jobs".to_string()),
        BufferDesc::storage(jobs_size),
    );
    let segments_buffer = builder.buffer(
        Some("large msdf segments".to_string()),
        BufferDesc::storage(segments_size),
    );
    let pixel_jobs_buffer = builder.buffer(
        Some("large msdf pixel jobs".to_string()),
        BufferDesc::storage(pixel_jobs_size),
    );
    let output = builder.texture(
        Some("large msdf output".to_string()),
        TextureDesc::render_target(scenario.target_size, wgpu::TextureFormat::Rgba8Unorm),
    );
    let sampler = builder.sampler(
        Some("large msdf sampler".to_string()),
        wgpu::SamplerDescriptor {
            label: Some("large msdf sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        },
    );
    let program = builder.program(ProgramDesc::Compute(ComputeProgramDesc {
        label: Some("large msdf generate".to_string()),
        shader: MSDF_GENERATOR_SHADER.to_string(),
        entry: "cs_main".to_string(),
        bind_groups: BindGroupLayoutSpec::single(vec![
            BindingLayoutEntry::uniform(0, wgpu::ShaderStages::COMPUTE),
            BindingLayoutEntry::storage_texture(
                1,
                wgpu::ShaderStages::COMPUTE,
                wgpu::TextureFormat::Rgba16Float,
                wgpu::StorageTextureAccess::WriteOnly,
            ),
            BindingLayoutEntry::storage(2, wgpu::ShaderStages::COMPUTE, true),
            BindingLayoutEntry::storage(3, wgpu::ShaderStages::COMPUTE, true),
            BindingLayoutEntry::storage(4, wgpu::ShaderStages::COMPUTE, true),
        ]),
    }));
    builder.compute_pass(ComputePassDesc {
        label: Some("large msdf generate".to_string()),
        owner: None,
        program,
        bindings: vec![
            Binding::uniform(0, 0, globals_buffer),
            Binding::storage_texture(0, 1, atlas_texture),
            Binding::storage_buffer(0, 2, jobs_buffer),
            Binding::storage_buffer(0, 3, segments_buffer),
            Binding::storage_buffer(0, 4, pixel_jobs_buffer),
        ],
        dispatch: Dispatch {
            x: atlas.msdf_pixel_count.div_ceil(64),
            y: 1,
            z: 1,
        }
        .into(),
    });
    let text_program = builder.program(ProgramDesc::Render(RenderProgramDesc {
        label: Some("large msdf render".to_string()),
        shader: MSDF_TEXT_SHADER.to_string(),
        vertex_entry: "vs_main".to_string(),
        fragment_entry: "fs_main".to_string(),
        bind_groups: BindGroupLayoutSpec::single(vec![
            BindingLayoutEntry::uniform(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
            BindingLayoutEntry::texture(1, wgpu::ShaderStages::FRAGMENT),
            BindingLayoutEntry::sampler(2, wgpu::ShaderStages::FRAGMENT),
            BindingLayoutEntry::storage(3, wgpu::ShaderStages::VERTEX, true),
        ]),
        targets: vec![Some(wgpu::ColorTargetState {
            format: wgpu::TextureFormat::Rgba8Unorm,
            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
            write_mask: wgpu::ColorWrites::ALL,
        })],
        vertex_buffers: Vec::new(),
        primitive: wgpu::PrimitiveState::default(),
    }));
    builder.render_pass(RenderPassDesc {
        label: Some("large msdf render".to_string()),
        owner: None,
        program: text_program,
        targets: vec![RenderTargetRef {
            texture: output,
            load: LoadOp::Clear(wgpu::Color::TRANSPARENT),
            store: wgpu::StoreOp::Store,
        }],
        bindings: vec![
            Binding::uniform(0, 0, text_globals_buffer),
            Binding::sampled_texture(0, 1, atlas_texture),
            Binding::sampler(0, 2, sampler),
            Binding::storage_buffer(0, 3, instances_buffer),
        ],
        vertex_buffers: Vec::new(),
        index_buffer: None,
        draw: DrawCommand::Draw(Draw {
            vertices: 0..6,
            instances: 0..atlas.glyph_count as u32,
        }),
        scissor: None,
    });
    let plan = builder.build();
    renderer.prepare_plan(&plan).unwrap();

    let globals = GpuMsdfGlobals {
        atlas_size: [scenario.atlas_size.width, scenario.atlas_size.height],
        job_count: atlas.jobs.len() as u32,
        dirty_pixel_count: atlas.msdf_pixel_count,
        _padding: [0; 2],
    };
    let mut update = FrameUpdate::new();
    let text_globals = GpuTextGlobals {
        target_size: [
            scenario.target_size.width as f32,
            scenario.target_size.height as f32,
        ],
        px_range: scenario.px_range as f32,
        glyph_count: atlas.glyph_count as u32,
    };
    let atlas_pixels = lumen_text::rgba8_to_rgba16_float(&atlas.pixels);
    update.write_texture_rgba16_float(
        atlas_texture,
        &atlas_pixels,
        scenario.atlas_size.width * 8,
        scenario.atlas_size.height,
    );
    update.write_buffer(globals_buffer, 0, bytemuck::bytes_of(&globals));
    update.write_buffer(text_globals_buffer, 0, bytemuck::bytes_of(&text_globals));
    update.write_buffer(instances_buffer, 0, bytemuck::cast_slice(&atlas.instances));
    update.write_buffer(jobs_buffer, 0, bytemuck::cast_slice(&atlas.jobs));
    update.write_buffer(segments_buffer, 0, bytemuck::cast_slice(&atlas.segments));
    update.write_buffer(
        pixel_jobs_buffer,
        0,
        bytemuck::cast_slice(&atlas.pixel_jobs),
    );
    renderer.execute(&plan, &update).unwrap();

    Some(MsdfRenderResult {
        renderer,
        atlas,
        atlas_texture,
        output,
        atlas_size: scenario.atlas_size,
        target_size: scenario.target_size,
    })
}

fn rendered_alpha_stats(
    renderer: &Renderer,
    output: TextureId,
    target_size: Size,
    rect: [u32; 4],
) -> (usize, usize) {
    let output_bytes = read_texture_rgba8(renderer, output, target_size);
    let mut opaque = 0usize;
    let mut total = 0usize;
    for y in rect[1]..rect[3] {
        for x in rect[0]..rect[2] {
            total += 1;
            let index = ((y * target_size.width + x) * 4 + 3) as usize;
            if output_bytes[index] > 240 {
                opaque += 1;
            }
        }
    }
    (opaque, total)
}

fn median3_f16(r: u16, g: u16, b: u16) -> f32 {
    let r = half::f16::from_bits(r).to_f32();
    let g = half::f16::from_bits(g).to_f32();
    let b = half::f16::from_bits(b).to_f32();
    r.min(g).max(r.max(g).min(b))
}

fn renderer() -> Option<Renderer> {
    match pollster::block_on(Renderer::new()) {
        Ok(renderer) => Some(renderer),
        Err(error) => {
            eprintln!("skipping GPU-backed lumen-text test: {error:#}");
            None
        }
    }
}

fn read_texture_rgba8(renderer: &Renderer, id: TextureId, size: Size) -> Vec<u8> {
    read_texture_bytes(renderer, id, size, 4)
}

fn read_texture_rgba16_float(renderer: &Renderer, id: TextureId, size: Size) -> Vec<u16> {
    let bytes = read_texture_bytes(renderer, id, size, 8);
    bytemuck::cast_slice(&bytes).to_vec()
}

fn read_texture_bytes(
    renderer: &Renderer,
    id: TextureId,
    size: Size,
    bytes_per_pixel: u32,
) -> Vec<u8> {
    let unpadded_bytes_per_row = size.width * bytes_per_pixel;
    let padded_bytes_per_row = align_to(unpadded_bytes_per_row, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let output_size = padded_bytes_per_row as u64 * size.height as u64;
    let output = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("lumen-text msdf readback"),
        size: output_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = renderer
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("lumen-text msdf readback encoder"),
        });
    encoder.copy_texture_to_buffer(
        renderer.texture(id).unwrap().as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &output,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(size.height),
            },
        },
        wgpu::Extent3d {
            width: size.width,
            height: size.height,
            depth_or_array_layers: 1,
        },
    );
    renderer.queue.submit([encoder.finish()]);

    let slice = output.slice(..);
    let (tx, rx) = mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
    renderer
        .device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    rx.recv().unwrap().unwrap();
    let padded = slice.get_mapped_range().to_vec();
    output.unmap();

    let mut unpadded = Vec::with_capacity((unpadded_bytes_per_row * size.height) as usize);
    for row in padded.chunks_exact(padded_bytes_per_row as usize) {
        unpadded.extend_from_slice(&row[..unpadded_bytes_per_row as usize]);
    }
    unpadded
}

fn write_texture_png(renderer: &Renderer, id: TextureId, size: Size, path: String) {
    let bytes = read_texture_rgba8(renderer, id, size);
    image::save_buffer(
        path,
        &bytes,
        size.width,
        size.height,
        image::ColorType::Rgba8,
    )
    .unwrap();
}

fn align_to(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}
