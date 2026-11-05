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
    recorder.upload();
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
