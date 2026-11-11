use lumen_engine_gpu::*;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const LAYOUT: &[LayoutEntry] = &[LayoutEntry::compute(0, LayoutKind::StorageTexture(FORMAT))];
const WHOLE: Dispatch = Dispatch { x: 1, y: 1, z: 1 };

fn setup() -> (Renderer, Recorder) {
    let renderer =
        pollster::block_on(Renderer::new()).expect("lumen-gpu tests need a wgpu adapter");
    let recorder = Recorder::new(renderer.device.clone(), renderer.queue.clone());
    (renderer, recorder)
}

/// A runtime program writing `value` to the red channel of texel `x`, so a pass that ran another
/// program shows it.
fn program_writing(recorder: &mut Recorder, value: u32, x: u32) -> ProgramRef {
    recorder
        .dynamic_compute_program(
            "test",
            u64::from(value) << 32 | u64::from(x),
            || {
                format!(
                    "@group(0) @binding(0) var output: texture_storage_2d<rgba8unorm, write>;\n\
                     @compute @workgroup_size(1) fn cs_main() {{\n\
                       textureStore(output, vec2<i32>({x}, 0), vec4<f32>({value}.0 / 255.0, 0.0, 0.0, 1.0));\n\
                     }}"
                )
            },
            "cs_main",
            LAYOUT,
        )
        .expect("program")
}

/// Submits the frame and returns the red channel of `size` texels of `target`.
fn submit_and_read(
    renderer: &Renderer,
    recorder: &mut Recorder,
    target: TexRef,
    size: Size,
) -> Vec<u8> {
    let output = recorder.resolve(target, None).expect("resolve");
    recorder.upload().expect("upload");
    recorder.submit().expect("submit");
    let rgba = renderer
        .read_texture_rgba8(recorder.texture(output), size)
        .expect("read");
    recorder.end_frame();
    rgba.chunks(4).map(|texel| texel[0]).collect()
}

#[test]
fn programs_a_frame_recorded_stay_until_it_is_submitted() {
    let (renderer, mut recorder) = setup();
    let size = Size::new(1, 1);
    let usage = TextureDesc::storage(size, FORMAT).usage;

    recorder.begin_frame();
    let from_this_frame = program_writing(&mut recorder, 7, 0);
    let target = recorder.acquire(size, FORMAT, usage);
    recorder.compute(from_this_frame, &[(0, Res::Tex(target))], WHOLE);
    for value in 100..164 {
        program_writing(&mut recorder, value, 0);
    }
    assert_eq!(submit_and_read(&renderer, &mut recorder, target, size), [7]);

    // Recording a program held from an earlier frame pins it just the same.
    recorder.begin_frame();
    let held = program_writing(&mut recorder, 9, 0);
    recorder.begin_frame();
    let target = recorder.acquire(size, FORMAT, usage);
    recorder.compute(held, &[(0, Res::Tex(target))], WHOLE);
    for value in 200..255 {
        program_writing(&mut recorder, value, 0);
    }
    for x in 1..10 {
        program_writing(&mut recorder, 255, x);
    }
    assert_eq!(submit_and_read(&renderer, &mut recorder, target, size), [9]);
}

#[test]
fn a_frame_using_more_programs_than_the_bound_renders_and_the_excess_goes_later() {
    let (renderer, mut recorder) = setup();
    const USED: u32 = 70;
    let size = Size::new(USED, 1);
    let usage = TextureDesc::storage(size, FORMAT).usage;
    let expected: Vec<u8> = (0..USED as u8).collect();

    for _ in 0..2 {
        recorder.begin_frame();
        let target = recorder.acquire(size, FORMAT, usage);
        for value in 0..USED {
            let program = program_writing(&mut recorder, value, value);
            recorder.compute(program, &[(0, Res::Tex(target))], WHOLE);
        }
        assert_eq!(
            submit_and_read(&renderer, &mut recorder, target, size),
            expected
        );
        assert_eq!(recorder.program_count(), USED as usize);
    }
    assert_eq!(recorder.stats().programs_created, u64::from(USED));

    recorder.begin_frame();
    let target = recorder.acquire(size, FORMAT, usage);
    let program = program_writing(&mut recorder, 0, 0);
    recorder.compute(program, &[(0, Res::Tex(target))], WHOLE);
    submit_and_read(&renderer, &mut recorder, target, size);
    assert_eq!(recorder.program_count(), 64);
}

#[test]
fn a_copy_keeps_each_texel_where_it_was() {
    let (renderer, mut recorder) = setup();
    let size = Size::new(2, 2);
    let source = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("copied"),
        size: size.as_extent(),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    renderer.queue.write_texture(
        source.as_image_copy(),
        &[10, 0, 0, 255, 20, 0, 0, 255, 30, 0, 0, 255, 40, 0, 0, 255],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(8),
            rows_per_image: Some(2),
        },
        size.as_extent(),
    );
    recorder.begin_frame();
    let source = recorder.adopt(std::sync::Arc::new(source));
    let usage = TextureDesc::storage(size, FORMAT).usage | wgpu::TextureUsages::COPY_DST;
    let target = recorder.acquire(size, FORMAT, usage);
    recorder.copy(source, target, [0, 0], size);
    assert_eq!(
        submit_and_read(&renderer, &mut recorder, target, size),
        [10, 20, 30, 40]
    );
}

#[test]
fn releasing_everything_frees_what_the_recorder_held_and_it_records_again() {
    let (renderer, mut recorder) = setup();
    let size = Size::new(1, 1);
    let usage = TextureDesc::storage(size, FORMAT).usage;
    let frame = |recorder: &mut Recorder| {
        recorder.begin_frame();
        let program = program_writing(recorder, 3, 0);
        let target = recorder.acquire(size, FORMAT, usage);
        recorder.uniform(&[0_u32; 4]);
        recorder.compute(program, &[(0, Res::Tex(target))], WHOLE);
        submit_and_read(&renderer, recorder, target, size)
    };
    assert_eq!(frame(&mut recorder), [3]);
    assert!(recorder.held_bytes() > 0);

    recorder.release_all();
    assert_eq!(recorder.held_bytes(), 0);
    assert_eq!(recorder.program_count(), 0);
    assert_eq!(recorder.bind_group_count(), 0);

    assert_eq!(frame(&mut recorder), [3]);
}

const DRAW_SHADER: &str = "
struct Params { value: f32, depth: f32 }
@group(0) @binding(0) var<uniform> params: Params;
@vertex fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(corner * 2.0 - 1.0, params.depth, 1.0);
}
@fragment fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(params.value / 255.0, 0.0, 0.0, 1.0);
}";
const DRAW_LAYOUT: &[LayoutEntry] = &[LayoutEntry::staged(
    0,
    wgpu::ShaderStages::VERTEX_FRAGMENT,
    LayoutKind::Uniform,
)];
const DRAW_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::RENDER_ATTACHMENT
    .union(wgpu::TextureUsages::TEXTURE_BINDING)
    .union(wgpu::TextureUsages::COPY_SRC);

/// A program covering the target with a red `value` at a depth, both given by its uniform.
fn draw_program(recorder: &mut Recorder, depth: Option<DepthTest>) -> ProgramRef {
    recorder
        .render_program(
            "test draw",
            DRAW_SHADER,
            "vs_main",
            "fs_main",
            DRAW_LAYOUT,
            RenderTargetSpec {
                depth,
                ..RenderTargetSpec::flat(FORMAT, wgpu::BlendState::REPLACE)
            },
            0,
        )
        .expect("program")
}

#[test]
fn a_draw_behind_what_is_already_drawn_is_hidden_unless_nothing_wrote_depth() {
    let (renderer, mut recorder) = setup();
    let size = Size::new(1, 1);
    let drawn = |recorder: &mut Recorder, near: DepthTest| {
        recorder.begin_frame();
        let target = recorder.acquire(size, FORMAT, DRAW_USAGE);
        let depth = recorder.acquire(size, DEPTH_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let draws = [(near, [10.0_f32, 0.25]), (DepthTest::Write, [20.0, 0.75])];
        for (index, (test, params)) in draws.into_iter().enumerate() {
            let program = draw_program(recorder, Some(test));
            let load = if index == 0 {
                TargetLoad::Clear
            } else {
                TargetLoad::Load
            };
            let bindings = [(0, recorder.uniform(&params))];
            recorder.draw_with_depth(program, &bindings, target, load, depth, load, 3, 1);
        }
        submit_and_read(&renderer, recorder, target, size)
    };
    assert_eq!(drawn(&mut recorder, DepthTest::Write), [10]);
    assert_eq!(drawn(&mut recorder, DepthTest::Read), [20]);
}

#[test]
fn a_draw_without_a_viewport_covers_the_target_after_one_with_a_viewport() {
    let (renderer, mut recorder) = setup();
    let size = Size::new(2, 1);
    recorder.begin_frame();
    let target = recorder.acquire(size, FORMAT, DRAW_USAGE);
    let program = draw_program(&mut recorder, None);
    let left = [(0, recorder.uniform(&[10.0_f32, 0.0]))];
    recorder.draw(
        program,
        &left,
        target,
        TargetLoad::Clear,
        3,
        1,
        Some([0.0, 0.0, 1.0, 1.0]),
    );
    let right = [(0, recorder.uniform(&[20.0_f32, 0.0]))];
    recorder.draw(
        program,
        &right,
        target,
        TargetLoad::Load,
        3,
        1,
        Some([1.0, 0.0, 1.0, 1.0]),
    );
    assert_eq!(
        submit_and_read(&renderer, &mut recorder, target, size),
        [10, 20]
    );

    recorder.begin_frame();
    let target = recorder.acquire(size, FORMAT, DRAW_USAGE);
    recorder.draw(
        program,
        &left,
        target,
        TargetLoad::Clear,
        3,
        1,
        Some([0.0, 0.0, 1.0, 1.0]),
    );
    let whole = [(0, recorder.uniform(&[30.0_f32, 0.0]))];
    recorder.draw(program, &whole, target, TargetLoad::Load, 3, 1, None);
    assert_eq!(
        submit_and_read(&renderer, &mut recorder, target, size),
        [30, 30]
    );
}

#[test]
fn a_render_program_binding_a_storage_texture_is_refused() {
    let (_renderer, mut recorder) = setup();
    const WRITING: &[LayoutEntry] = &[LayoutEntry::staged(
        0,
        wgpu::ShaderStages::FRAGMENT,
        LayoutKind::StorageTexture(FORMAT),
    )];
    let program = recorder.render_program(
        "test draw",
        DRAW_SHADER,
        "vs_main",
        "fs_main",
        WRITING,
        RenderTargetSpec::flat(FORMAT, wgpu::BlendState::REPLACE),
        0,
    );
    assert!(program.is_err());
}

const DEPTH_SHADER: &str = "
struct Params { value: f32, depth: f32 }
@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var map: texture_depth_2d;
@group(0) @binding(2) var compare: sampler_comparison;
@vertex fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(corner * 2.0 - 1.0, params.depth, 1.0);
}
@fragment fn fs_depth() {}
@fragment fn fs_compare() -> @location(0) vec4<f32> {
    let lit = textureSampleCompareLevel(map, compare, vec2<f32>(0.5), params.depth);
    return vec4<f32>(lit, 0.0, 0.0, 1.0);
}";

#[test]
fn a_depth_only_draw_writes_depth_a_later_draw_compares_against() {
    const WRITE_LAYOUT: &[LayoutEntry] = &[LayoutEntry::staged(
        0,
        wgpu::ShaderStages::VERTEX_FRAGMENT,
        LayoutKind::Uniform,
    )];
    const COMPARE_LAYOUT: &[LayoutEntry] = &[
        LayoutEntry::staged(0, wgpu::ShaderStages::VERTEX_FRAGMENT, LayoutKind::Uniform),
        LayoutEntry::staged(1, wgpu::ShaderStages::FRAGMENT, LayoutKind::DepthTexture),
        LayoutEntry::staged(
            2,
            wgpu::ShaderStages::FRAGMENT,
            LayoutKind::ComparisonSampler,
        ),
    ];
    let (renderer, mut recorder) = setup();
    let size = Size::new(1, 1);
    let write = recorder
        .render_program(
            "test depth",
            DEPTH_SHADER,
            "vs_main",
            "fs_depth",
            WRITE_LAYOUT,
            RenderTargetSpec::depth_only(DepthTest::Write, None),
            0,
        )
        .expect("program");
    let compare = recorder
        .render_program(
            "test compare",
            DEPTH_SHADER,
            "vs_main",
            "fs_compare",
            COMPARE_LAYOUT,
            RenderTargetSpec::flat(FORMAT, wgpu::BlendState::REPLACE),
            0,
        )
        .expect("program");
    let sampler = recorder.sampler(1, || wgpu::SamplerDescriptor {
        compare: Some(wgpu::CompareFunction::LessEqual),
        ..wgpu::SamplerDescriptor::default()
    });
    let lit = |recorder: &mut Recorder, reference: f32| {
        recorder.begin_frame();
        let map = recorder.acquire(
            size,
            DEPTH_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let at_half = [(0, recorder.uniform(&[0.0_f32, 0.5]))];
        recorder.draw_depth(write, &at_half, map, TargetLoad::Clear, 3, 1);
        let target = recorder.acquire(size, FORMAT, DRAW_USAGE);
        let bindings = [
            (0, recorder.uniform(&[0.0_f32, reference])),
            (1, Res::Tex(map)),
            (2, Res::Sampler(sampler)),
        ];
        recorder.draw(compare, &bindings, target, TargetLoad::Clear, 3, 1, None);
        submit_and_read(&renderer, recorder, target, size)
    };
    assert_eq!(lit(&mut recorder, 0.25), [255]);
    assert_eq!(lit(&mut recorder, 0.75), [0]);
}

#[test]
fn a_frame_with_more_storage_than_the_device_takes_fails_to_upload() {
    let (_renderer, mut recorder) = setup();
    recorder.limit_storage(1 << 16, 1 << 17);

    recorder.begin_frame();
    recorder.storage_parts(&[&[0u8; (1 << 16) + 1]]);
    let error = recorder.upload().expect_err("one binding over the limit");
    assert!(error.to_string().contains("doesn't fit the GPU"), "{error}");
    assert!(matches!(
        error.downcast_ref::<FrameError>(),
        Some(FrameError::TooLarge(_))
    ));

    recorder.begin_frame();
    for _ in 0..3 {
        recorder.storage_parts(&[&[0u8; 1 << 16]]);
    }
    recorder.upload().expect_err("the buffer over its limit");

    recorder.begin_frame();
    recorder.storage_parts(&[&[0u8; 1 << 16]]);
    recorder.storage_parts(&[&[0u8; 1 << 16]]);
    recorder.upload().expect("a frame within both limits");
}
