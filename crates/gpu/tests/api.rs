use lumen_engine_gpu::*;

const SHADER: &str = r#"
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
    return vec4<f32>(1.0, 0.0, 0.0, 1.0);
}
"#;

#[test]
fn texture_domains_preserve_storage_display_and_data_bounds() {
    let size = Size::new(1920, 1080);
    let desc = TextureDesc {
        domain: TextureDomain {
            storage_size: size,
            display_rect: RectI::new(-10, 20, 640, 360),
            data_rect: RectI::new(0, 0, 1280, 720),
        },
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
    };

    assert_eq!(desc.domain.storage_size, size);
    assert_eq!(desc.domain.display_rect, RectI::new(-10, 20, 640, 360));
    assert_eq!(desc.domain.data_rect, RectI::new(0, 0, 1280, 720));
}

#[test]
fn resource_descriptors_include_expected_gpu_usages() {
    let size = Size::new(16, 8);
    let target = TextureDesc::render_target(size, wgpu::TextureFormat::Rgba8Unorm);
    let sampled = TextureDesc::sampled(size, wgpu::TextureFormat::Rgba8Unorm);
    let storage = TextureDesc::storage(size, wgpu::TextureFormat::Rgba8Unorm);

    assert!(
        target
            .usage
            .contains(wgpu::TextureUsages::RENDER_ATTACHMENT)
    );
    assert!(target.usage.contains(wgpu::TextureUsages::TEXTURE_BINDING));
    assert!(target.usage.contains(wgpu::TextureUsages::COPY_SRC));
    assert!(sampled.usage.contains(wgpu::TextureUsages::COPY_DST));
    assert!(sampled.usage.contains(wgpu::TextureUsages::COPY_SRC));
    assert!(storage.usage.contains(wgpu::TextureUsages::STORAGE_BINDING));
    assert!(storage.usage.contains(wgpu::TextureUsages::TEXTURE_BINDING));

    assert!(
        BufferDesc::uniform(64)
            .usage
            .contains(wgpu::BufferUsages::UNIFORM)
    );
    assert!(
        BufferDesc::storage(64)
            .usage
            .contains(wgpu::BufferUsages::STORAGE)
    );
}

#[test]
fn builder_assigns_stable_ids() {
    let mut builder = RenderPlan::builder();
    let tex_a = builder.texture(
        Some("input".to_string()),
        TextureDesc::sampled(Size::new(4, 4), wgpu::TextureFormat::Rgba8Unorm),
    );
    let tex_b = builder.texture(
        Some("node-output".to_string()),
        TextureDesc::render_target(Size::new(4, 4), wgpu::TextureFormat::Rgba8Unorm),
    );
    let buf_a = builder.buffer(None, BufferDesc::uniform(16));
    let buf_b = builder.buffer(Some("params".to_string()), BufferDesc::storage(32));
    let program = builder.program(render_program_desc());
    let pass = builder.render_pass(RenderPassDesc {
        label: Some("paint".to_string()),
        program,
        targets: vec![RenderTargetRef {
            texture: tex_b,
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

    assert_eq!(tex_a, TextureId(0));
    assert_eq!(tex_b, TextureId(1));
    assert_eq!(buf_a, BufferId(0));
    assert_eq!(buf_b, BufferId(1));
    assert_eq!(program, ProgramId(0));
    assert_eq!(pass, PassId(0));
    assert_eq!(plan.passes()[0].id, pass);
}

#[test]
fn plan_records_pass_order_across_render_compute_and_copy() {
    let mut builder = RenderPlan::builder();
    let size = Size::new(8, 8);
    let a = builder.texture(
        None,
        TextureDesc::storage(size, wgpu::TextureFormat::Rgba8Unorm),
    );
    let b = builder.texture(
        None,
        TextureDesc::render_target(size, wgpu::TextureFormat::Rgba8Unorm),
    );
    let render = builder.program(render_program_desc());
    let compute = builder.program(ProgramDesc::Compute(ComputeProgramDesc {
        label: Some("compute".to_string()),
        shader: "@compute @workgroup_size(1) fn cs_main() {}".to_string(),
        entry: "cs_main".to_string(),
        bind_groups: Vec::new(),
    }));

    builder.compute_pass(ComputePassDesc {
        label: Some("first".to_string()),
        program: compute,
        bindings: Vec::new(),
        dispatch: Dispatch { x: 1, y: 1, z: 1 },
    });
    builder.render_pass(RenderPassDesc {
        label: Some("second".to_string()),
        program: render,
        targets: vec![RenderTargetRef {
            texture: b,
            load: LoadOp::Load,
            store: wgpu::StoreOp::Store,
        }],
        bindings: Vec::new(),
        draw: Draw {
            vertices: 0..3,
            instances: 0..1,
        },
        viewport: None,
    });
    builder.copy_texture(CopyTextureDesc {
        label: Some("third".to_string()),
        source: b,
        destination: a,
        origin: wgpu::Origin3d::ZERO,
        size,
    });
    let plan = builder.build();

    assert!(matches!(plan.passes()[0].desc, PassDesc::Compute(_)));
    assert!(matches!(plan.passes()[1].desc, PassDesc::Render(_)));
    assert!(matches!(plan.passes()[2].desc, PassDesc::CopyTexture(_)));
}

#[test]
fn frame_update_records_buffer_and_texture_uploads_in_order() {
    let buffer_bytes = [1, 2, 3, 4];
    let texture_bytes = [255; 16];
    let mut update = FrameUpdate::new();
    update
        .write_buffer(BufferId(3), 8, &buffer_bytes)
        .write_texture_rgba8(TextureId(2), &texture_bytes, 8, 2);

    assert_eq!(update.uploads().len(), 2);
    match &update.uploads()[0] {
        Upload::Buffer { id, offset, data } => {
            assert_eq!(*id, BufferId(3));
            assert_eq!(*offset, 8);
            assert_eq!(*data, buffer_bytes);
        }
        Upload::TextureRgba8 { .. }
        | Upload::TextureRgba8Region { .. }
        | Upload::TextureRgba16Float { .. } => {
            panic!("expected buffer upload first")
        }
    }
    match &update.uploads()[1] {
        Upload::TextureRgba8 {
            id,
            data,
            bytes_per_row,
            rows_per_image,
        } => {
            assert_eq!(*id, TextureId(2));
            assert_eq!(*data, texture_bytes);
            assert_eq!(*bytes_per_row, 8);
            assert_eq!(*rows_per_image, 2);
        }
        Upload::Buffer { .. }
        | Upload::TextureRgba8Region { .. }
        | Upload::TextureRgba16Float { .. } => {
            panic!("expected texture upload second")
        }
    }
}

#[test]
fn binding_helpers_record_group_binding_and_access_kind() {
    assert_eq!(
        Binding::sampled_texture(1, 2, TextureId(3)).resource,
        BindingResource::Texture {
            id: TextureId(3),
            access: TextureAccess::Sampled,
        }
    );
    assert_eq!(
        Binding::storage_texture(1, 2, TextureId(3)).resource,
        BindingResource::Texture {
            id: TextureId(3),
            access: TextureAccess::Storage,
        }
    );
    assert_eq!(
        Binding::uniform(0, 4, BufferId(5)).resource,
        BindingResource::Buffer {
            id: BufferId(5),
            access: BufferAccess::Uniform,
        }
    );
    assert_eq!(
        Binding::storage_buffer(0, 4, BufferId(5)).resource,
        BindingResource::Buffer {
            id: BufferId(5),
            access: BufferAccess::Storage,
        }
    );
    assert_eq!(
        Binding::sampler(2, 9, SamplerId(11)).resource,
        BindingResource::Sampler(SamplerId(11))
    );
}

fn render_program_desc() -> ProgramDesc {
    ProgramDesc::Render(RenderProgramDesc {
        label: Some("render".to_string()),
        shader: SHADER.to_string(),
        vertex_entry: "vs_main".to_string(),
        fragment_entry: "fs_main".to_string(),
        bind_groups: Vec::new(),
        targets: vec![Some(wgpu::ColorTargetState {
            format: wgpu::TextureFormat::Rgba8Unorm,
            blend: Some(wgpu::BlendState::REPLACE),
            write_mask: wgpu::ColorWrites::ALL,
        })],
        primitive: wgpu::PrimitiveState::default(),
    })
}
