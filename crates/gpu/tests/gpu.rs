use std::sync::{Arc, mpsc};

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

const STAGE_WRITE_SHADER: &str = r#"
@group(0) @binding(0) var tex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(1) var<storage, read_write> data: array<u32, 4>;

@compute @workgroup_size(1, 1, 1)
fn cs_main() {
    textureStore(tex, vec2<i32>(0, 0), vec4<f32>(0.0, 1.0, 0.0, 1.0));
    data[0] = 7u;
}
"#;

const STAGE_READ_SHADER: &str = r#"
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var<storage, read> data: array<u32, 4>;
@group(0) @binding(2) var<storage, read_write> result: array<u32, 4>;

@compute @workgroup_size(1, 1, 1)
fn cs_main() {
    result[0] = u32(round(textureLoad(tex, vec2<i32>(0, 0), 0).g * 255.0));
    result[1] = data[0] * 2u;
}
"#;

const STAGE_DISPATCH_SHADER: &str = r#"
@group(0) @binding(0) var<storage, read_write> out: array<u32, 4>;

@compute @workgroup_size(1, 1, 1)
fn cs_main(@builtin(workgroup_id) id: vec3<u32>) {
    out[id.x] = 100u + id.x;
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
fn compute_pass_writes_storage_texture_and_rebinds_a_replacement() {
    let mut renderer = renderer();
    let size = Size::new(4, 4);
    let desc = TextureDesc::storage(size, wgpu::TextureFormat::Rgba8Unorm);
    let mut builder = RenderPlan::builder();
    let output = builder.texture(Some("output".to_string()), desc);
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
        program,
        bindings: vec![Binding::storage_texture(0, 0, output)],
        dispatch: Dispatch { x: 1, y: 1, z: 1 },
    });
    let plan = builder.build();
    renderer.prepare_plan(&plan).unwrap();
    renderer.execute(&plan, &FrameUpdate::new()).unwrap();
    let bytes = read_texture_rgba8(&renderer, output, size);
    assert_rgba8_near(&bytes[0..4], [64, 128, 191, 255]);

    let replacement = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("replacement output"),
        size: size.as_extent(),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: desc.usage,
        view_formats: &[],
    });
    renderer.replace_texture(output, replacement, desc).unwrap();
    let bytes = read_texture_rgba8(&renderer, output, size);
    assert_eq!(&bytes[0..4], &[0, 0, 0, 0], "replacement starts blank");
    renderer.execute(&plan, &FrameUpdate::new()).unwrap();

    let bytes = read_texture_rgba8(&renderer, output, size);
    assert_rgba8_near(&bytes[0..4], [64, 128, 191, 255]);
}

#[test]
fn uniform_upload_updates_compute_shader_output_without_replanning() {
    let mut renderer = renderer();
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
        program,
        bindings: vec![
            Binding::uniform(0, 0, uniforms),
            Binding::storage_texture(0, 1, output),
        ],
        dispatch: Dispatch { x: 1, y: 1, z: 1 },
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
    let mut second_update = FrameUpdate::new();
    second_update.write_buffer(uniforms, 0, bytemuck::cast_slice(&second));
    renderer.execute(&plan, &second_update).unwrap();
    assert_eq!(
        &read_texture_rgba8(&renderer, output, size)[0..4],
        &[255, 0, 0, 255]
    );

    // A write through the exposed buffer is not seen by the renderer, so the same upload
    // repeated afterwards must still be written.
    renderer.queue.write_buffer(
        renderer.buffer(uniforms).unwrap(),
        0,
        bytemuck::cast_slice(&first),
    );
    renderer.execute(&plan, &second_update).unwrap();
    assert_eq!(
        &read_texture_rgba8(&renderer, output, size)[0..4],
        &[255, 0, 0, 255]
    );
}

#[test]
fn skipped_passes_leave_their_targets_as_last_written() {
    let mut renderer = renderer();
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
        program,
        bindings: vec![
            Binding::uniform(0, 0, uniforms),
            Binding::storage_texture(0, 1, output),
        ],
        dispatch: Dispatch { x: 1, y: 1, z: 1 },
    });
    let plan = builder.build();
    renderer.prepare_plan(&plan).unwrap();
    let write = |renderer: &mut Renderer, color: [f32; 4], skip: &[bool]| {
        let mut update = FrameUpdate::new();
        update.write_buffer(uniforms, 0, bytemuck::cast_slice(&color));
        renderer.apply_frame_update(&plan, &update).unwrap();
        renderer.submit_plan_skipping(&plan, skip).unwrap();
        read_texture_rgba8(renderer, output, size)[0..4].to_vec()
    };

    assert_eq!(
        write(&mut renderer, [0.0, 1.0, 0.0, 1.0], &[]),
        [0, 255, 0, 255]
    );
    assert_eq!(
        write(&mut renderer, [1.0, 0.0, 0.0, 1.0], &[true]),
        [0, 255, 0, 255]
    );
    assert_eq!(
        write(&mut renderer, [1.0, 0.0, 0.0, 1.0], &[false]),
        [255, 0, 0, 255]
    );
}

#[test]
fn consecutive_compute_passes_share_one_pass_across_skipped_passes() {
    let mut renderer = renderer();
    let size = Size::new(1, 1);
    let compute = wgpu::ShaderStages::COMPUTE;
    let storage =
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST;
    let mut builder = RenderPlan::builder();
    let texture = builder.texture(
        Some("stage".to_string()),
        TextureDesc::storage(size, wgpu::TextureFormat::Rgba8Unorm),
    );
    let copied = builder.texture(
        Some("copied".to_string()),
        TextureDesc::sampled(size, wgpu::TextureFormat::Rgba8Unorm),
    );
    let buffer = |builder: &mut RenderPlanBuilder, label: &str, size, usage| {
        builder.buffer(Some(label.to_string()), BufferDesc { size, usage })
    };
    let data = buffer(&mut builder, "data", 16, storage);
    let result = buffer(&mut builder, "result", 16, storage);
    let out = buffer(&mut builder, "out", 16, storage);
    let program = |builder: &mut RenderPlanBuilder, shader: &str, entries| {
        builder.program(ProgramDesc::Compute(ComputeProgramDesc {
            label: None,
            shader: shader.to_string(),
            entry: "cs_main".to_string(),
            bind_groups: BindGroupLayoutSpec::single(entries),
        }))
    };
    let write = program(
        &mut builder,
        STAGE_WRITE_SHADER,
        vec![
            BindingLayoutEntry::storage_texture(
                0,
                compute,
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::StorageTextureAccess::WriteOnly,
            ),
            BindingLayoutEntry::storage(1, compute, false),
        ],
    );
    let read = program(
        &mut builder,
        STAGE_READ_SHADER,
        vec![
            BindingLayoutEntry::texture(0, compute),
            BindingLayoutEntry::storage(1, compute, true),
            BindingLayoutEntry::storage(2, compute, false),
        ],
    );
    let dispatch_program = program(
        &mut builder,
        STAGE_DISPATCH_SHADER,
        vec![BindingLayoutEntry::storage(0, compute, false)],
    );
    let dispatch = |label: &str, program, bindings, dispatch| ComputePassDesc {
        label: Some(label.to_string()),
        program,
        bindings,
        dispatch,
    };
    let one = Dispatch { x: 1, y: 1, z: 1 };
    builder.compute_pass(dispatch(
        "write",
        write,
        vec![
            Binding::storage_texture(0, 0, texture),
            Binding::storage_buffer(0, 1, data),
        ],
        one,
    ));
    builder.compute_pass(dispatch(
        "read",
        read,
        vec![
            Binding::sampled_texture(0, 0, texture),
            Binding::storage_buffer(0, 1, data),
            Binding::storage_buffer(0, 2, result),
        ],
        one,
    ));
    builder.copy_texture(CopyTextureDesc {
        label: Some("copy".to_string()),
        source: texture,
        destination: copied,
        origin: wgpu::Origin3d::ZERO,
        size,
    });
    builder.compute_pass(dispatch(
        "dispatch",
        dispatch_program,
        vec![Binding::storage_buffer(0, 0, out)],
        Dispatch { x: 3, y: 1, z: 1 },
    ));
    let plan = builder.build();
    renderer.prepare_plan(&plan).unwrap();

    let check = |renderer: &Renderer, copied_texel: [u8; 4]| {
        assert_eq!(read_buffer_u32(renderer, data, 4), [7, 0, 0, 0]);
        assert_eq!(read_buffer_u32(renderer, result, 4), [255, 14, 0, 0]);
        assert_eq!(read_buffer_u32(renderer, out, 4), [100, 101, 102, 0]);
        assert_eq!(
            read_texture_rgba8(renderer, copied, size)[0..4],
            copied_texel
        );
    };

    renderer.submit_plan(&plan).unwrap();
    check(&renderer, [0, 255, 0, 255]);

    let zeros = [0_u8; 16];
    let mut clear = FrameUpdate::new();
    for id in [data, result, out] {
        clear.write_buffer(id, 0, &zeros);
    }
    clear.write_texture_rgba8(copied, &zeros[..4], 4, 1);
    renderer.apply_frame_update(&plan, &clear).unwrap();
    renderer
        .submit_plan_skipping(&plan, &[false, false, true, false])
        .unwrap();
    check(&renderer, [0, 0, 0, 0]);
}

#[test]
fn sibling_renderers_never_accept_an_invalid_program() {
    let root = renderer();
    let mut builder = RenderPlan::builder();
    builder.program(ProgramDesc::Compute(ComputeProgramDesc {
        label: Some("invalid".to_string()),
        shader: "@compute @workgroup_size(1) fn cs_main() { missing(); }".to_string(),
        entry: "cs_main".to_string(),
        bind_groups: vec![],
    }));
    let plan = std::sync::Arc::new(builder.build());
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let threads: Vec<_> = (0..3)
        .map(|_| {
            let mut renderer = root.sibling();
            let (plan, barrier) = (plan.clone(), barrier.clone());
            std::thread::spawn(move || {
                (0..200)
                    .filter(|_| {
                        barrier.wait();
                        renderer.prepare_plan(&plan).is_ok()
                    })
                    .count()
            })
        })
        .collect();
    let accepted: usize = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .sum();
    assert_eq!(accepted, 0);
}

#[test]
fn render_pass_draws_to_its_target_or_to_an_external_texture() {
    let mut renderer = renderer();
    let size = Size::new(2, 2);
    let desc = TextureDesc::render_target(size, wgpu::TextureFormat::Rgba8Unorm);
    let mut builder = RenderPlan::builder();
    let output = builder.texture(Some("output".to_string()), desc);
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
        primitive: wgpu::PrimitiveState::default(),
    }));
    builder.render_pass(RenderPassDesc {
        label: Some("draw external".to_string()),
        program,
        targets: vec![RenderTargetRef {
            texture: output,
            load: LoadOp::Clear(wgpu::Color::BLACK),
            store: wgpu::StoreOp::Store,
        }],
        bindings: Vec::new(),
        draw: Draw {
            vertices: 0..3,
            instances: 0..1,
        },
        viewport: None,
    });
    let plan = builder.build();
    renderer.prepare_plan(&plan).unwrap();

    let original = renderer.texture_arc(output).unwrap();
    let external = Arc::new(renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("external render target"),
        size: size.as_extent(),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: desc.usage,
        view_formats: &[],
    }));
    let submitted = renderer
        .submit_plan_with_external_texture_skipping(
            &plan,
            output,
            ExternalTexture::new(Arc::clone(&external), desc.domain),
            &[],
        )
        .unwrap();

    assert!(Arc::ptr_eq(
        &renderer.texture_arc(output).unwrap(),
        &original
    ));
    assert!(std::ptr::eq(submitted.texture(), external.as_ref()));
    submitted.wait(&renderer.device).unwrap();
    let bytes = read_wgpu_texture_rgba8(&renderer, submitted.texture(), size);
    assert_rgba8_near(&bytes[0..4], [255, 128, 0, 255]);
    let bytes = read_texture_rgba8(&renderer, output, size);
    assert_eq!(&bytes[0..4], &[0, 0, 0, 0], "internal target is untouched");

    renderer.execute(&plan, &FrameUpdate::new()).unwrap();
    let bytes = read_texture_rgba8(&renderer, output, size);
    assert_rgba8_near(&bytes[0..4], [255, 128, 0, 255]);
}

#[test]
fn copy_texture_pass_copies_uploaded_texture_data() {
    let mut renderer = renderer();
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
    assert_eq!(&bytes[8..16], &[70, 80, 90, 255, 100, 110, 120, 255]);
}

#[test]
fn execute_rejects_unprepared_or_changed_plans() {
    let mut renderer = renderer();
    let one_buffer = || {
        let mut builder = RenderPlan::builder();
        builder.buffer(None, BufferDesc::uniform(16));
        builder.build()
    };
    let (prepared, same_shape) = (one_buffer(), one_buffer());

    let unprepared = renderer
        .execute(&prepared, &FrameUpdate::new())
        .unwrap_err();
    assert!(
        unprepared.to_string().contains("not been prepared"),
        "unexpected error: {unprepared}"
    );

    renderer.prepare_plan(&prepared).unwrap();
    assert!(renderer.execute(&prepared, &FrameUpdate::new()).is_ok());
    let err = renderer
        .execute(&same_shape, &FrameUpdate::new())
        .unwrap_err();
    assert!(
        err.to_string().contains("not been prepared"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejected_buffer_upload_is_not_cached_as_written() {
    let mut renderer = renderer();
    let mut builder = RenderPlan::builder();
    let buffer = builder.buffer(None, BufferDesc::uniform(16));
    let plan = builder.build();
    renderer.prepare_plan(&plan).unwrap();

    // Not a multiple of 4 bytes, so the write fails validation.
    let bytes = [1, 2, 3];
    let mut update = FrameUpdate::new();
    update.write_buffer(buffer, 0, &bytes);
    for _ in 0..2 {
        let err = renderer.execute(&plan, &update).unwrap_err();
        assert!(
            err.to_string().contains("validation"),
            "unexpected error: {err}"
        );
    }
}

#[test]
fn execute_rejects_buffer_uploads_past_declared_size() {
    let mut renderer = renderer();
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

fn renderer() -> Renderer {
    pollster::block_on(Renderer::new()).expect("lumen-gpu tests need a wgpu adapter")
}

fn read_buffer_u32(renderer: &Renderer, id: BufferId, count: usize) -> Vec<u32> {
    let size = (count * 4) as u64;
    let output = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("lumen-gpu test buffer readback"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = renderer
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("lumen-gpu test buffer readback encoder"),
        });
    encoder.copy_buffer_to_buffer(renderer.buffer(id).unwrap(), 0, &output, 0, size);
    renderer.queue.submit([encoder.finish()]);

    let slice = output.slice(..);
    let (tx, rx) = mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
    renderer
        .device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    rx.recv().unwrap().unwrap();
    let words = bytemuck::cast_slice(&slice.get_mapped_range().unwrap()).to_vec();
    output.unmap();
    words
}

fn read_texture_rgba8(renderer: &Renderer, id: TextureId, size: Size) -> Vec<u8> {
    read_wgpu_texture_rgba8(renderer, renderer.texture(id).unwrap(), size)
}

fn read_wgpu_texture_rgba8(renderer: &Renderer, texture: &wgpu::Texture, size: Size) -> Vec<u8> {
    renderer.read_texture_rgba8(texture, size).unwrap()
}
