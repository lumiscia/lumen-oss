use std::sync::mpsc;

use lumen_engine_gpu::*;

const COMPUTE_TEXTURE_SHADER: &str = r#"
@group(0) @binding(0) var output_tex: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= 4u || id.y >= 4u) {
        return;
    }
    textureStore(output_tex, vec2<i32>(id.xy), vec4<f32>(0.25, 0.5, 0.75, 1.0));
}
"#;

const UNIFORM_COMPUTE_SHADER: &str = r#"
struct Color {
    value: vec4<f32>,
}

@group(0) @binding(0) var<uniform> color: Color;
@group(0) @binding(1) var output_tex: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(1, 1, 1)
fn cs_main() {
    textureStore(output_tex, vec2<i32>(0, 0), color.value);
}
"#;

const RENDER_SHADER: &str = r#"
@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4<f32> {
    let positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0)
    );
    return vec4<f32>(positions[vertex], 0.0, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 0.5, 0.0, 1.0);
}
"#;

fn assert_rgba8_near(actual: &[u8], expected: [u8; 4]) {
    assert!(
        actual.len() >= 4,
        "expected at least 4 bytes, got {}",
        actual.len()
    );

    for (channel, (&actual_channel, expected)) in
        actual[..4].iter().zip(expected.iter()).enumerate()
    {
        let delta = (i16::from(actual_channel) - i16::from(*expected)).abs();
        assert!(
            delta <= 1,
            "channel {channel}: actual rgba={:?}, expected rgba={expected:?}",
            &actual[..4],
        );
    }
}

#[test]
fn compute_pass_writes_storage_texture() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let size = Size::new(4, 4);
    let mut builder = RenderPlan::builder();
    let output = builder.texture(
        Some("output".to_string()),
        TextureDesc::storage(size, wgpu::TextureFormat::Rgba8Unorm),
    );
    let program = builder.program(ProgramDesc::Compute(ComputeProgramDesc {
        label: Some("fill texture".to_string()),
        shader: COMPUTE_TEXTURE_SHADER.to_string(),
        entry: "cs_main".to_string(),
        bind_groups: BindGroupLayoutSpec::single(vec![BindingLayoutEntry::storage_texture(
            0,
            wgpu::ShaderStages::COMPUTE,
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::StorageTextureAccess::WriteOnly,
        )]),
    }));
    builder.compute_pass(ComputePassDesc {
        label: Some("fill".to_string()),
        owner: None,
        program,
        bindings: vec![Binding::storage_texture(0, 0, output)],
        dispatch: Dispatch { x: 1, y: 1, z: 1 }.into(),
    });
    let plan = builder.build();

    renderer.prepare_plan(&plan).unwrap();
    renderer.execute(&plan, &FrameUpdate::new()).unwrap();

    let bytes = read_texture_rgba8(&renderer, output, size);
    assert_rgba8_near(&bytes[0..4], [64, 128, 191, 255]);
}

#[test]
fn uniform_upload_updates_compute_shader_output_without_replanning() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let size = Size::new(1, 1);
    let mut builder = RenderPlan::builder();
    let uniforms = builder.buffer(Some("color".to_string()), BufferDesc::uniform(16));
    let output = builder.texture(
        Some("output".to_string()),
        TextureDesc::storage(size, wgpu::TextureFormat::Rgba8Unorm),
    );
    let program = builder.program(ProgramDesc::Compute(ComputeProgramDesc {
        label: Some("uniform color".to_string()),
        shader: UNIFORM_COMPUTE_SHADER.to_string(),
        entry: "cs_main".to_string(),
        bind_groups: BindGroupLayoutSpec::single(vec![
            BindingLayoutEntry::uniform(0, wgpu::ShaderStages::COMPUTE),
            BindingLayoutEntry::storage_texture(
                1,
                wgpu::ShaderStages::COMPUTE,
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::StorageTextureAccess::WriteOnly,
            ),
        ]),
    }));
    builder.compute_pass(ComputePassDesc {
        label: Some("write color".to_string()),
        owner: Some(NodeKey(1)),
        program,
        bindings: vec![
            Binding::uniform(0, 0, uniforms),
            Binding::storage_texture(0, 1, output),
        ],
        dispatch: Dispatch { x: 1, y: 1, z: 1 }.into(),
    });
    let plan = builder.build();
    renderer.prepare_plan(&plan).unwrap();

    let first = [0.0_f32, 1.0, 0.0, 1.0];
    let mut update = FrameUpdate::new();
    update.write_buffer(uniforms, 0, bytemuck::cast_slice(&first));
    renderer.execute(&plan, &update).unwrap();
    assert_eq!(
        &read_texture_rgba8(&renderer, output, size)[0..4],
        &[0, 255, 0, 255]
    );

    let second = [1.0_f32, 0.0, 0.0, 1.0];
    let mut update = FrameUpdate::new();
    update.write_buffer(uniforms, 0, bytemuck::cast_slice(&second));
    renderer.execute(&plan, &update).unwrap();
    assert_eq!(
        &read_texture_rgba8(&renderer, output, size)[0..4],
        &[255, 0, 0, 255]
    );
}

#[test]
fn render_pass_draws_fullscreen_triangle_to_target() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let size = Size::new(2, 2);
    let mut builder = RenderPlan::builder();
    let output = builder.texture(
        Some("output".to_string()),
        TextureDesc::render_target(size, wgpu::TextureFormat::Rgba8Unorm),
    );
    let program = builder.program(ProgramDesc::Render(RenderProgramDesc {
        label: Some("orange".to_string()),
        shader: RENDER_SHADER.to_string(),
        vertex_entry: "vs_main".to_string(),
        fragment_entry: "fs_main".to_string(),
        bind_groups: Vec::new(),
        targets: vec![Some(wgpu::ColorTargetState {
            format: wgpu::TextureFormat::Rgba8Unorm,
            blend: Some(wgpu::BlendState::REPLACE),
            write_mask: wgpu::ColorWrites::ALL,
        })],
        vertex_buffers: Vec::new(),
        primitive: wgpu::PrimitiveState::default(),
    }));
    builder.render_pass(RenderPassDesc {
        label: Some("draw".to_string()),
        owner: None,
        program,
        targets: vec![RenderTargetRef {
            texture: output,
            load: LoadOp::Clear(wgpu::Color::BLACK),
            store: wgpu::StoreOp::Store,
        }],
        bindings: Vec::new(),
        vertex_buffers: Vec::new(),
        index_buffer: None,
        draw: DrawCommand::Draw(Draw {
            vertices: 0..3,
            instances: 0..1,
        }),
        scissor: None,
    });
    let plan = builder.build();

    renderer.prepare_plan(&plan).unwrap();
    renderer.execute(&plan, &FrameUpdate::new()).unwrap();

    let bytes = read_texture_rgba8(&renderer, output, size);
    assert_rgba8_near(&bytes[0..4], [255, 128, 0, 255]);
}

#[test]
fn copy_texture_pass_copies_uploaded_texture_data() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let size = Size::new(2, 2);
    let mut builder = RenderPlan::builder();
    let source = builder.texture(
        Some("source".to_string()),
        TextureDesc::sampled(size, wgpu::TextureFormat::Rgba8Unorm),
    );
    let destination = builder.texture(
        Some("destination".to_string()),
        TextureDesc::sampled(size, wgpu::TextureFormat::Rgba8Unorm),
    );
    builder.copy_texture(CopyTextureDesc {
        label: Some("copy".to_string()),
        source,
        destination,
        origin: wgpu::Origin3d::ZERO,
        size,
    });
    let plan = builder.build();
    renderer.prepare_plan(&plan).unwrap();

    let data = [
        10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120, 255,
    ];
    let mut update = FrameUpdate::new();
    update.write_texture_rgba8(source, &data, 8, 2);
    renderer.execute(&plan, &update).unwrap();

    let bytes = read_texture_rgba8(&renderer, destination, size);
    assert_eq!(&bytes[0..8], &[10, 20, 30, 255, 40, 50, 60, 255]);
    assert_eq!(&bytes[256..264], &[70, 80, 90, 255, 100, 110, 120, 255]);
}

#[test]
fn execute_rejects_unprepared_or_changed_plans() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let plan = RenderPlan::builder().build();

    assert!(renderer.execute(&plan, &FrameUpdate::new()).is_ok());

    let mut changed = RenderPlan::builder();
    changed.buffer(None, BufferDesc::uniform(16));
    let changed = changed.build();
    let err = renderer.execute(&changed, &FrameUpdate::new()).unwrap_err();
    assert!(
        err.to_string().contains("not been prepared"),
        "unexpected error: {err}"
    );
}

#[test]
fn execute_rejects_buffer_uploads_past_declared_size() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let mut builder = RenderPlan::builder();
    let buffer = builder.buffer(None, BufferDesc::uniform(4));
    let plan = builder.build();
    renderer.prepare_plan(&plan).unwrap();

    let bytes = [1, 2, 3, 4, 5];
    let mut update = FrameUpdate::new();
    update.write_buffer(buffer, 0, &bytes);

    let err = renderer.execute(&plan, &update).unwrap_err();
    assert!(
        err.to_string().contains("exceeds declared buffer size"),
        "unexpected error: {err}"
    );
}

fn renderer() -> Option<Renderer> {
    match pollster::block_on(Renderer::new()) {
        Ok(renderer) => Some(renderer),
        Err(error) => {
            eprintln!("skipping GPU-backed lumen-gpu test: {error:#}");
            None
        }
    }
}

fn read_texture_rgba8(renderer: &Renderer, id: TextureId, size: Size) -> Vec<u8> {
    let bytes_per_pixel = 4;
    let unpadded_bytes_per_row = size.width * bytes_per_pixel;
    let padded_bytes_per_row = align_to(unpadded_bytes_per_row, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let output_size = padded_bytes_per_row as u64 * size.height as u64;
    let output = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("lumen-gpu test readback"),
        size: output_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = renderer
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("lumen-gpu test readback encoder"),
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
    let bytes = slice.get_mapped_range().to_vec();
    output.unmap();
    bytes
}

fn align_to(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}
