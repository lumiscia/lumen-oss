use lumen_engine::node::source::text_content::TextContentDelegate;
use lumen_engine::{
    composition::{Composition, CompositionBuilder, RenderSettings, TimelineSettings},
    expr::Expression,
    gpu::{CompileContext, FrameBindContext},
    graph::Graph,
    media::{
        CpuMediaFrame, ImageMetadata, ImageResolver, MediaFrame, MediaStore, VideoFrameResolver,
        VideoMetadata,
    },
    node::{
        Deferred, NodeId, NodeKind, PortRef, ShaderUniforms,
        compositing::raster_multimerge::RasterMultiMerge,
        media_output::MediaOutput,
        processing::{
            crop::{Crop, CropParamsDelegate},
            memo::{Memo, MemoParamsDelegate},
            opacity::Opacity,
            resize::{Resize, ResizeMode, ResizeParamsDelegate, ResizeSampling},
            wgsl_shader::{WgslShader, WgslShaderParamsDelegate},
        },
        source::{
            background::{Background, BackgroundParamsDelegate},
            media_in::{MediaIn, MediaInParamsDelegate},
            text::{Text, TextAlignmentHorizontal, TextAlignmentVertical, TextParamsDelegate},
        },
        vector::{
            paint::Paint,
            path::{Path, PathParamsDelegate},
            shape::{Shape, ShapeParamsDelegate},
        },
    },
};
use serde_json::json;
use std::sync::{Arc, mpsc};

static TEXT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

// Vulkan devices from separate wgpu instances can still crash inside the loader when used from
// several threads at once, even with HAL labels discarded, so GPU tests run one at a time. The
// lock is reentrant per thread for tests that hold two renderers, and is released only when the
// last holder on that thread drops.
static GPU_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

thread_local! {
    static GPU_LOCK_HELD: std::cell::RefCell<(usize, Option<std::sync::MutexGuard<'static, ()>>)> =
        const { std::cell::RefCell::new((0, None)) };
}

/// Not `Send`: the count and guard it releases belong to the thread that acquired it.
struct GpuTestLock(std::marker::PhantomData<std::rc::Rc<()>>);

impl GpuTestLock {
    fn acquire() -> Self {
        GPU_LOCK_HELD.with(|held| {
            let mut held = held.borrow_mut();
            if held.0 == 0 {
                held.1 = Some(GPU_LOCK.lock().unwrap_or_else(|error| error.into_inner()));
            }
            held.0 += 1;
        });
        Self(std::marker::PhantomData)
    }
}

impl Drop for GpuTestLock {
    fn drop(&mut self) {
        GPU_LOCK_HELD.with(|held| {
            let mut held = held.borrow_mut();
            held.0 -= 1;
            if held.0 == 0 {
                held.1 = None;
            }
        });
    }
}

struct TestRenderer {
    renderer: lumen_engine::gpu::GpuCompositionRenderer,
    _lock: GpuTestLock,
}

impl std::ops::Deref for TestRenderer {
    type Target = lumen_engine::gpu::GpuCompositionRenderer;

    fn deref(&self) -> &Self::Target {
        &self.renderer
    }
}

impl std::ops::DerefMut for TestRenderer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.renderer
    }
}

/// A job's fonts: one file under one family name, in a text session of its own.
#[derive(Debug)]
struct JobFont {
    family: &'static str,
    id: String,
    data: Vec<u8>,
    session: lumen_engine::media::TextSession,
}

impl JobFont {
    fn new(family: &'static str, id: &str, data: Vec<u8>) -> Self {
        Self {
            family,
            id: id.to_string(),
            data,
            session: Default::default(),
        }
    }
}

struct JobFontResolver {
    id: String,
    data: Vec<u8>,
}

impl lumen_engine::media::FontResolver for JobFontResolver {
    fn id(&self) -> &str {
        &self.id
    }

    fn data(&self) -> Result<Vec<Vec<u8>>, lumen_engine::error::MediaError> {
        Ok(vec![self.data.clone()])
    }
}

impl MediaStore for JobFont {
    fn get_image_resolver(&self, _: &str) -> Option<Box<dyn ImageResolver>> {
        None
    }

    fn get_video_resolver(&self, _: &str) -> Option<Box<dyn VideoFrameResolver>> {
        None
    }

    fn font_resolver_id(&self, family: &str) -> Option<String> {
        (family == self.family).then(|| self.id.clone())
    }

    fn get_font_resolver(
        &self,
        family: &str,
    ) -> Option<Box<dyn lumen_engine::media::FontResolver>> {
        (family == self.family).then(|| {
            Box::new(JobFontResolver {
                id: self.id.clone(),
                data: self.data.clone(),
            }) as _
        })
    }

    fn text_session(&self) -> Option<&lumen_engine::media::TextSession> {
        Some(&self.session)
    }
}

/// `font` with the family name `family`, which fits in its names' space.
fn font_named(font: &[u8], family: &str) -> Vec<u8> {
    let mut font = font.to_vec();
    let read = |font: &[u8], at: usize| u16::from_be_bytes([font[at], font[at + 1]]) as usize;
    let tables = read(&font, 4);
    let name = font[12..12 + tables * 16]
        .as_chunks::<16>()
        .0
        .iter()
        .find(|table| &table[..4] == b"name")
        .unwrap();
    let table = u32::from_be_bytes(name[8..12].try_into().unwrap()) as usize;
    let strings = table + read(&font, table + 4);
    let encoded: Vec<u8> = family.encode_utf16().flat_map(u16::to_be_bytes).collect();
    for record in 0..read(&font, table + 2) {
        let at = table + 6 + record * 12;
        if read(&font, at) == 3 && matches!(read(&font, at + 6), 1 | 16) {
            let start = strings + read(&font, at + 10);
            font[start..start + encoded.len()].copy_from_slice(&encoded);
            font[at + 8..at + 10].copy_from_slice(&(encoded.len() as u16).to_be_bytes());
        }
    }
    font
}

#[test]
fn a_family_only_one_job_has_does_not_reach_another_jobs_measurements() {
    let measure = |job: &JobFont| {
        Expression::parse("text_width(\"WWW\", 32, 0, \"OnlyIn\")")
            .unwrap()
            .evaluate(&lumen_engine::expr::ExpressionContext {
                media: Some(job),
                ..Default::default()
            })
            .unwrap()
            .as_f64()
            .unwrap()
    };
    let with_font = JobFont::new(
        "OnlyIn",
        "job-font-only-in-one",
        ScaledFontStore::new("OnlyIn", 2).data,
    );
    let without = JobFont::new("Absent", "job-font-absent", Vec::new());

    let fallback = measure(&without);
    let own = measure(&with_font);
    assert!(
        own > fallback * 1.5,
        "{own} should differ from fallback {fallback}"
    );
    assert_eq!(measure(&without), fallback);
    assert_eq!(measure(&with_font), own);
}

#[test]
fn jobs_draw_one_family_name_with_their_own_font_files() {
    const ROBOTO: &[u8] = lumen_text::ROBOTO_REGULAR_BYTES;
    const INTER: &[u8] = include_bytes!("../../text/assets/inter/InterVariable-Latin.ttf");

    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    // What is drawn and where both depend on the font: measuring must read the job's too.
    let composition = |family: &str| {
        text_test_composition(TextParamsDelegate {
            content: TextContentDelegate::Text(Deferred::value("Rag Wave 28".to_string())),
            font_family: Deferred::value(family.to_string()),
            font_size: Deferred::value(64.0),
            position: Deferred::Expr(
                Expression::parse(&format!(
                    "vec2(text_width(\"WWWWWW\", 32, 0, \"{family}\"), 40)"
                ))
                .unwrap(),
            ),
            ..Default::default()
        })
    };
    let jobs = || {
        [
            JobFont::new("Face", "job-font-roboto", font_named(ROBOTO, "Face")),
            JobFont::new("Face", "job-font-inter", font_named(INTER, "Face")),
        ]
    };
    let shared_family = composition("Face");

    // The true faces under their own names, which is how each file must draw under the shared one.
    let roboto = render_rgba8(&mut renderer, &composition("Roboto"), 0, &EmptyMediaStore);
    let inter = render_rgba8(
        &mut gpu_composition_renderer().unwrap(),
        &composition("Inter"),
        0,
        &JobFont::new("Inter", "job-font-true-inter", INTER.to_vec()),
    );
    assert!(roboto.as_chunks::<4>().0.iter().any(|pixel| pixel[3] > 0));
    assert_ne!(roboto, inter);
    let expected = [&roboto, &inter];

    // In turns on one renderer, whose plans and atlas outlive each store's frame.
    let turns = jobs();
    for job in [0, 1, 0, 1, 1, 0] {
        let pixels = render_rgba8(&mut renderer, &shared_family, 0, &turns[job]);
        assert!(pixels == *expected[job], "job {job} drew another's font");
    }

    // Files alike in all but size number their glyphs alike, so their atlas keys match.
    let sized = |scale: u16| {
        JobFont::new(
            "Sizing",
            &format!("job-font-sized-{scale}"),
            ScaledFontStore::new("Sizing", scale).data,
        )
    };
    let [large, larger] = [sized(1), sized(2)];
    let glyphs = text_test_composition(TextParamsDelegate {
        content: TextContentDelegate::Text(Deferred::value("W".to_string())),
        font_family: Deferred::value("Sizing".to_string()),
        font_size: Deferred::value(64.0),
        position: Deferred::value((16.0, 40.0)),
        ..Default::default()
    });
    let alone = [&sized(1), &sized(2)]
        .map(|job| render_rgba8(&mut gpu_composition_renderer().unwrap(), &glyphs, 0, job));
    assert_ne!(alone[0], alone[1]);
    for (job, expected) in [
        (&large, &alone[0]),
        (&larger, &alone[1]),
        (&large, &alone[0]),
    ] {
        let pixels = render_rgba8(&mut renderer, &glyphs, 0, job);
        assert!(pixels == *expected, "{} drew another's glyphs", job.id);
    }

    // At once in one process, each on its own renderer as `lumen-server` runs jobs.
    let together = jobs();
    std::thread::scope(|scope| {
        for (job, expected) in together.iter().zip(expected) {
            let shared_family = &shared_family;
            scope.spawn(move || {
                let mut renderer =
                    pollster::block_on(lumen_engine::gpu::GpuCompositionRenderer::new()).unwrap();
                for _ in 0..8 {
                    let pixels = render_rgba8(&mut renderer, shared_family, 0, job);
                    assert!(pixels == *expected, "{} drew another's font", job.id);
                }
            });
        }
    });
}

/// A job whose fonts change as it runs.
#[derive(Debug, Default)]
struct ChangingFonts {
    fonts: std::sync::Mutex<std::collections::HashMap<String, (String, Vec<u8>)>>,
    session: lumen_engine::media::TextSession,
}

impl ChangingFonts {
    fn set(&self, family: &str, id: &str, data: Vec<u8>) {
        let font = (id.to_string(), data);
        self.fonts.lock().unwrap().insert(family.to_string(), font);
    }
}

impl MediaStore for ChangingFonts {
    fn get_image_resolver(&self, _: &str) -> Option<Box<dyn ImageResolver>> {
        None
    }

    fn get_video_resolver(&self, _: &str) -> Option<Box<dyn VideoFrameResolver>> {
        None
    }

    fn font_resolver_id(&self, family: &str) -> Option<String> {
        Some(self.fonts.lock().unwrap().get(family)?.0.clone())
    }

    fn get_font_resolver(
        &self,
        family: &str,
    ) -> Option<Box<dyn lumen_engine::media::FontResolver>> {
        let (id, data) = self.fonts.lock().unwrap().get(family)?.clone();
        Some(Box::new(JobFontResolver { id, data }))
    }

    fn text_session(&self) -> Option<&lumen_engine::media::TextSession> {
        Some(&self.session)
    }
}

#[test]
fn replacing_a_font_mid_job_leaves_no_stale_glyph_on_screen() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let glyph = text_test_composition(TextParamsDelegate {
        content: TextContentDelegate::Text(Deferred::value("W".to_string())),
        font_family: Deferred::value("Sizing".to_string()),
        font_size: Deferred::value(64.0),
        position: Deferred::value((16.0, 40.0)),
        ..Default::default()
    });
    let data = |scale: u16| ScaledFontStore::new("Sizing", scale).data;
    let alone = [1, 2].map(|scale| {
        let job = JobFont::new("Sizing", &format!("alone-{scale}"), data(scale));
        render_rgba8(&mut gpu_composition_renderer().unwrap(), &glyph, 0, &job)
    });
    assert_ne!(alone[0], alone[1]);

    // Rebuilt fonts number their faces alike and these their glyphs alike, so only the rebuild
    // tells one font's atlas keys from the other's.
    let job = ChangingFonts::default();
    for (turn, scale) in [1, 2, 1].into_iter().enumerate() {
        job.set("Sizing", &format!("turn-{turn}"), data(scale));
        let pixels = render_rgba8(&mut renderer, &glyph, 0, &job);
        assert!(pixels == alone[usize::from(scale) - 1], "turn {turn}");
    }

    // Only the merge asks for this font, once the text has placed its glyphs. It goes by the name
    // the text draws in, and being the latest load it takes over the text.
    let both = lumen_engine::json::parse_value(&json!({
        "timeline": { "fps": 10, "duration_frames": 10 },
        "render_settings": { "width": 256, "height": 128 },
        "nodes": [
            {
                "id": 1,
                "type": "text",
                "params": {
                    "content": "W",
                    "font_family": "Sizing",
                    "font_size": 64,
                    "position": [16, 40]
                }
            },
            {
                "id": 2,
                "type": "text",
                "params": {
                    "content": "W",
                    "font_family": "Sizing",
                    "font_size": 64,
                    "position": [128, 40]
                }
            },
            {
                "id": 3,
                "type": "merge",
                "params": { "opacity": "=min(text_width(\"W\", 32, 0, \"Second\"), 1)" }
            },
            { "id": 4, "type": "media_output" }
        ],
        "connections": [
            { "from_node": 1, "to_node": 3, "to_port": "base" },
            { "from_node": 2, "to_node": 3, "to_port": "overlay" },
            { "from_node": 3, "to_node": 4, "to_port": "source" }
        ]
    }))
    .unwrap();
    let job = ChangingFonts::default();
    job.set("Sizing", "both-sizing", data(1));
    job.set("Second", "both-second", data(2));
    let mut renderer = gpu_composition_renderer().unwrap();
    let first = render_rgba8(&mut renderer, &both, 0, &job);
    let settled = render_rgba8(&mut renderer, &both, 0, &job);
    assert!(first == settled);
    assert!(first.as_chunks::<4>().0.iter().any(|pixel| pixel[3] > 0));
}

fn text_test_composition(params: TextParamsDelegate) -> Composition {
    let mut graph = Graph::new();
    graph.nodes.insert(
        NodeId::new(1),
        NodeKind::Text(Text {
            id: NodeId::new(1),
            params,
        }),
    );
    graph.nodes.insert(
        NodeId::new(2),
        NodeKind::MediaOutput(MediaOutput {
            id: NodeId::new(2),
            source: PortRef::new(NodeId::new(1), "output".to_string()),
        }),
    );
    let mut composition = test_composition(graph).into_builder();
    composition.render_settings.width = 1024;
    composition.render_settings.height = 256;
    let composition = composition.build().unwrap();
    composition
}

#[test]
fn text_at_fractional_position_draws_glyphs_on_whole_pixels() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let text = NodeId::new(1);
    let output = NodeId::new(2);
    let mut graph = Graph::new();
    graph.nodes.insert(
        text,
        NodeKind::Text(Text {
            id: text,
            params: TextParamsDelegate {
                content: TextContentDelegate::Text(Deferred::value(
                    "Sharp text\nwraps across lines".to_string(),
                )),
                font_size: Deferred::value(17.0),
                max_width: Deferred::value(90.0),
                position: Deferred::value((10.3, 20.6)),
                alignment_horizontal: TextAlignmentHorizontal::Center.into(),
                alignment_vertical: TextAlignmentVertical::Middle.into(),
                ..Default::default()
            },
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(text, "output".to_string()),
        }),
    );
    let mut composition = test_composition(graph).into_builder();
    composition.render_settings.width = 160;
    composition.render_settings.height = 80;
    let composition = composition.build().unwrap();

    let compiled = CompileContext::new(&composition).compile().unwrap();
    let bound = FrameBindContext::new(&composition, 0)
        .bind(&compiled)
        .unwrap();
    let instances_label = format!("text:{}:instances", text.0);
    let instances_buffer = compiled
        .plan
        .buffers()
        .iter()
        .find(|buffer| buffer.label.as_deref() == Some(instances_label.as_str()))
        .unwrap()
        .id;
    let update = bound.frame_update();
    let instances = update
        .uploads()
        .iter()
        .find_map(|upload| match upload {
            lumen_gpu::Upload::Buffer { id, data, .. } if *id == instances_buffer => Some(*data),
            _ => None,
        })
        .unwrap();
    let rects: Vec<[f32; 4]> = instances
        .as_chunks::<{ std::mem::size_of::<lumen_text::GpuGlyphInstance>() }>()
        .0
        .iter()
        .map(|instance| bytemuck::pod_read_unaligned(&instance[..16]))
        .collect();

    assert!(rects.len() > 10);
    assert!(
        rects
            .iter()
            .all(|rect| rect[0].fract() == 0.0 && rect[1].fract() == 0.0),
        "glyph quads must start on whole pixels: {rects:?}"
    );
}

/// Text drawn in a font whose glyphs are `scale` times the size of Roboto's, held back until the
/// media store resolves it.
#[derive(Debug, Clone)]
struct ScaledFontStore {
    family: &'static str,
    data: Vec<u8>,
}

impl ScaledFontStore {
    fn new(family: &'static str, scale: u16) -> Self {
        assert_eq!(family.len(), "Roboto".len());
        let mut data = lumen_text::ROBOTO_REGULAR_BYTES.to_vec();
        let utf16 = |name: &str| {
            name.encode_utf16()
                .flat_map(u16::to_be_bytes)
                .collect::<Vec<_>>()
        };
        for (old, new) in [
            (b"Roboto".to_vec(), family.as_bytes().to_vec()),
            (utf16("Roboto"), utf16(family)),
        ] {
            for offset in 0..=data.len() - old.len() {
                if data[offset..offset + old.len()] == old[..] {
                    data[offset..offset + old.len()].copy_from_slice(&new);
                }
            }
        }
        let table_count = u16::from_be_bytes(data[4..6].try_into().unwrap()) as usize;
        let head = data[12..12 + table_count * 16]
            .as_chunks::<16>()
            .0
            .iter()
            .find(|table| &table[..4] == b"head")
            .unwrap();
        let units_offset = u32::from_be_bytes(head[8..12].try_into().unwrap()) as usize + 18;
        let units = u16::from_be_bytes(data[units_offset..units_offset + 2].try_into().unwrap());
        data[units_offset..units_offset + 2].copy_from_slice(&(units / scale).to_be_bytes());
        Self { family, data }
    }
}

impl lumen_engine::media::FontResolver for ScaledFontStore {
    fn id(&self) -> &str {
        self.family
    }

    fn data(&self) -> Result<Vec<Vec<u8>>, lumen_engine::error::MediaError> {
        Ok(vec![self.data.clone()])
    }
}

impl MediaStore for ScaledFontStore {
    fn get_image_resolver(&self, _: &str) -> Option<Box<dyn ImageResolver>> {
        None
    }

    fn get_video_resolver(&self, _: &str) -> Option<Box<dyn VideoFrameResolver>> {
        None
    }

    fn font_resolver_id(&self, family: &str) -> Option<String> {
        (family == self.family).then(|| self.family.to_string())
    }

    fn get_font_resolver(
        &self,
        family: &str,
    ) -> Option<Box<dyn lumen_engine::media::FontResolver>> {
        (family == self.family).then(|| Box::new(self.clone()) as _)
    }
}

/// Alpha channel per pixel of an 8x4 render, as rows of 8 characters.
fn alpha_rows(pixels: &[u8]) -> Vec<String> {
    pixels
        .as_chunks::<4>()
        .0
        .chunks(8)
        .map(|row| {
            row.iter()
                .map(|pixel| if pixel[3] == 255 { '#' } else { '.' })
                .collect()
        })
        .collect()
}

fn single_node_composition(node: NodeKind, source_width: u32, source_height: u32) -> Composition {
    let background = NodeId::new(1);
    let output = NodeId::new(3);
    let mut graph = Graph::new();
    graph.nodes.insert(
        background,
        NodeKind::Background(Background {
            id: background,
            params: BackgroundParamsDelegate {
                paint: lumen_engine::node::vector::paint::PaintDelegate::from(
                    lumen_engine::node::vector::paint::Paint::solid([255, 0, 0, 255]),
                ),
                width: Deferred::value(source_width),
                height: Deferred::value(source_height),
                paint_supersample: Deferred::value(true),
            },
        }),
    );
    let node_id = NodeId::new(2);
    graph.nodes.insert(node_id, node);
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(node_id, "output".to_string()),
        }),
    );
    test_composition(graph)
}

#[test]
fn crop_accepts_sizes_beyond_i32() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let crop = single_node_composition(
        NodeKind::Crop(Crop {
            id: NodeId::new(2),
            params: CropParamsDelegate {
                x: Deferred::value(0),
                y: Deferred::value(0),
                width: Deferred::value(1 << 31),
                height: Deferred::value(i64::from(u32::MAX)),
            },
            source: PortRef::new(NodeId::new(1), "output".to_string()),
        }),
        8,
        4,
    );
    assert_eq!(
        alpha_rows(&render_rgba8(&mut renderer, &crop, 0, &EmptyMediaStore)),
        ["########"; 4]
    );
}

#[derive(Debug)]
struct EmptyMediaStore;

impl MediaStore for EmptyMediaStore {
    fn get_image_resolver(&self, _source: &str) -> Option<Box<dyn ImageResolver>> {
        None
    }

    fn get_video_resolver(&self, _stream_id: &str) -> Option<Box<dyn VideoFrameResolver>> {
        None
    }
}

fn gpu_composition_renderer() -> Option<TestRenderer> {
    let lock = GpuTestLock::acquire();
    match pollster::block_on(lumen_engine::gpu::GpuCompositionRenderer::new()) {
        Ok(renderer) => Some(TestRenderer {
            renderer,
            _lock: lock,
        }),
        Err(error) => {
            eprintln!("skipping GPU-backed lumen-engine test: {error:#}");
            None
        }
    }
}

fn render_rgba8(
    renderer: &mut lumen_engine::gpu::GpuCompositionRenderer,
    composition: &Composition,
    frame: u32,
    media: &impl MediaStore,
) -> Vec<u8> {
    let raster = renderer.render_frame(composition, frame, media).unwrap();
    read_rgba8(renderer.gpu_renderer(), raster)
}

fn read_rgba8(gpu: &lumen_gpu::Renderer, raster: lumen_engine::gpu::RasterHandle) -> Vec<u8> {
    read_texture_rgba8(
        gpu,
        gpu.texture(raster.texture).unwrap(),
        raster.domain.storage_size,
    )
}

fn read_texture_rgba8(
    gpu: &lumen_gpu::Renderer,
    texture: &lumen_gpu::wgpu::Texture,
    size: lumen_gpu::Size,
) -> Vec<u8> {
    let bytes_per_pixel = 4;
    let unpadded_bytes_per_row = size.width * bytes_per_pixel;
    let padded_bytes_per_row = unpadded_bytes_per_row
        .div_ceil(lumen_gpu::wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        * lumen_gpu::wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let output = gpu
        .device
        .create_buffer(&lumen_gpu::wgpu::BufferDescriptor {
            label: Some("lumen-engine path fill test readback"),
            size: u64::from(padded_bytes_per_row) * u64::from(size.height),
            usage: lumen_gpu::wgpu::BufferUsages::COPY_DST
                | lumen_gpu::wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
    let mut encoder =
        gpu.device
            .create_command_encoder(&lumen_gpu::wgpu::CommandEncoderDescriptor {
                label: Some("lumen-engine path fill test readback encoder"),
            });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        lumen_gpu::wgpu::TexelCopyBufferInfo {
            buffer: &output,
            layout: lumen_gpu::wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(size.height),
            },
        },
        size.as_extent(),
    );
    gpu.queue.submit([encoder.finish()]);

    let slice = output.slice(..);
    let (tx, rx) = mpsc::channel();
    slice.map_async(lumen_gpu::wgpu::MapMode::Read, move |result| {
        tx.send(result).unwrap();
    });
    gpu.device
        .poll(lumen_gpu::wgpu::PollType::wait_indefinitely())
        .unwrap();
    rx.recv().unwrap().unwrap();
    let mapped = slice.get_mapped_range().unwrap();
    let mut pixels = vec![0; unpadded_bytes_per_row as usize * size.height as usize];
    for row in 0..size.height as usize {
        let source = row * padded_bytes_per_row as usize;
        let destination = row * unpadded_bytes_per_row as usize;
        pixels[destination..destination + unpadded_bytes_per_row as usize]
            .copy_from_slice(&mapped[source..source + unpadded_bytes_per_row as usize]);
    }
    drop(mapped);
    output.unmap();
    pixels
}

fn test_composition(graph: Graph) -> Composition {
    CompositionBuilder::new(
        graph,
        TimelineSettings {
            fps: 24.0,
            duration_frames: 12,
        },
        RenderSettings {
            width: 8,
            height: 4,
            background_color: [0, 0, 0, 255],
        },
    )
    .build()
    .expect("valid test composition")
}

#[derive(Debug, Clone)]
struct TestMediaStore {
    id: String,
    frame: Arc<CpuMediaFrame>,
}

impl MediaStore for TestMediaStore {
    fn get_image_resolver(&self, source: &str) -> Option<Box<dyn ImageResolver>> {
        (source == self.id).then(|| Box::new(self.clone()) as Box<dyn ImageResolver>)
    }

    fn get_video_resolver(&self, stream_id: &str) -> Option<Box<dyn VideoFrameResolver>> {
        (stream_id == self.id).then(|| Box::new(self.clone()) as Box<dyn VideoFrameResolver>)
    }
}

impl ImageResolver for TestMediaStore {
    fn id(&self) -> &str {
        &self.id
    }

    fn metadata(&self) -> ImageMetadata {
        ImageMetadata {
            width: self.frame.width,
            height: self.frame.height,
        }
    }

    fn frame(&self) -> Result<MediaFrame, lumen_engine::error::MediaError> {
        Ok(MediaFrame::CpuRgba(Arc::clone(&self.frame)))
    }
}

impl VideoFrameResolver for TestMediaStore {
    fn id(&self) -> &str {
        &self.id
    }

    fn metadata(&self) -> VideoMetadata {
        VideoMetadata {
            width: self.frame.width,
            height: self.frame.height,
            frame_count: 1,
            fps: 24.0,
        }
    }

    fn frame(&self, _frame: u32) -> Result<MediaFrame, lumen_engine::error::MediaError> {
        Ok(MediaFrame::CpuRgba(Arc::clone(&self.frame)))
    }
}

fn render_drawable_graph(
    renderer: &mut lumen_engine::gpu::GpuCompositionRenderer,
    nodes: Vec<NodeKind>,
    size: u32,
) -> Vec<u8> {
    render_drawable_frame(renderer, nodes, size, 0, &EmptyMediaStore)
}

fn render_drawable_frame(
    renderer: &mut lumen_engine::gpu::GpuCompositionRenderer,
    nodes: Vec<NodeKind>,
    size: u32,
    frame: u32,
    media: &impl MediaStore,
) -> Vec<u8> {
    let mut graph = Graph::new();
    let mut last = NodeId::new(0);
    for node in nodes {
        last = node.id();
        graph.nodes.insert(last, node);
    }
    let output = NodeId::new(100);
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(last, "output".to_string()),
        }),
    );
    let mut composition = test_composition(graph).into_builder();
    composition.render_settings.width = size;
    composition.render_settings.height = size;
    let composition = composition.build().unwrap();
    renderer
        .compile_with_media(
            &composition,
            media,
            lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
        )
        .unwrap();
    render_rgba8(renderer, &composition, frame, media)
}

#[test]
fn malformed_path_data_fails_the_frame_naming_the_data_field() {
    let mut graph = Graph::new();
    graph.nodes.insert(
        NodeId::new(1),
        NodeKind::Path(Path {
            id: NodeId::new(1),
            params: PathParamsDelegate {
                data: Deferred::value("M 0 0 L 10 10 C nope".to_string()),
                ..Default::default()
            },
        }),
    );
    graph.nodes.insert(
        NodeId::new(2),
        NodeKind::MediaOutput(MediaOutput {
            id: NodeId::new(2),
            source: PortRef::new(NodeId::new(1), "output".to_string()),
        }),
    );
    let composition = test_composition(graph);
    let compiled = CompileContext::new(&composition).compile().unwrap();
    let error = FrameBindContext::new(&composition, 0)
        .bind(&compiled)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("Path node") && error.contains("`data` is malformed"),
        "{error}"
    );
}

#[test]
fn text_past_the_instance_limit_is_a_render_error_that_counts_drawn_glyphs() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    // The animated size gives the text the full limit, and the constant one a limit of its own.
    for font_size in [Deferred::value(8.0), expression("8 + frame * 0")] {
        let composition = |content: String| {
            let mut composition = text_test_composition(TextParamsDelegate {
                content: TextContentDelegate::Text(Deferred::value(content)),
                font_size: font_size.clone(),
                ..Default::default()
            })
            .into_builder();
            composition.render_settings.width = 64;
            composition.render_settings.height = 64;
            composition.build().unwrap()
        };
        // Whitespace is not drawn, so it does not count toward the limit.
        let spaced = composition(format!("ab{}", " ".repeat(3000)));
        let pixels = render_rgba8(&mut renderer, &spaced, 0, &EmptyMediaStore);
        assert!(pixels.as_chunks::<4>().0.iter().any(|pixel| pixel[3] > 0));

        let long = composition("a".repeat(2049));
        let error = renderer
            .render_frame(&long, 0, &EmptyMediaStore)
            .expect_err("2049 glyphs exceed the limit")
            .to_string();
        assert!(
            error.contains("Text node 1") && error.contains("2049") && error.contains("2048"),
            "{error}"
        );
    }
}

#[test]
fn text_decorations_split_by_span_paint_fit_the_size_counted_at_compile() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let composition = lumen_engine::json::parse_value(&json!({
        "timeline": { "fps": 24, "duration_frames": 1 },
        "render_settings": { "width": 64, "height": 32 },
        "nodes": [
            { "id": 1, "type": "text", "params": {
                "content": [
                    { "text": "a", "color": "#ff0000" },
                    { "text": "b", "color": "#0000ff" }
                ],
                "font_size": 24,
                "position": [4, 2],
                "underline": {}
            } },
            { "id": 2, "type": "media_output" }
        ],
        "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }]
    }))
    .unwrap();
    let pixels = render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);
    let has = |color: [u8; 3]| {
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[3] == 255 && pixel[..3] == color)
    };
    assert!(has([255, 0, 0]) && has([0, 0, 255]));
}

/// Inter with its family renamed to `name`, which must have the five letters of "Inter".
fn renamed_inter(name: &str) -> Vec<u8> {
    assert_eq!(name.len(), 5);
    let mut font = include_bytes!("../../text/assets/inter/InterVariable-Latin.ttf").to_vec();
    let utf16 = |text: &str| {
        text.encode_utf16()
            .flat_map(u16::to_be_bytes)
            .collect::<Vec<_>>()
    };
    for (from, to) in [
        ("Inter".as_bytes().to_vec(), name.as_bytes().to_vec()),
        (utf16("Inter"), utf16(name)),
    ] {
        let mut at = 0;
        while let Some(found) = font[at..].windows(from.len()).position(|w| w == from) {
            font[at + found..at + found + to.len()].copy_from_slice(&to);
            at += found + to.len();
        }
    }
    font
}

/// Resolves "Loader" to a font named "Qzxvj" and "Qzxvj" to no font data.
#[derive(Debug, Clone)]
struct LateFont;

impl lumen_engine::media::FontResolver for LateFont {
    fn id(&self) -> &str {
        "late-font"
    }
    fn data(&self) -> Result<Vec<Vec<u8>>, lumen_engine::error::MediaError> {
        Ok(vec![renamed_inter("Qzxvj")])
    }
}

#[derive(Debug, Clone)]
struct NoFont;

impl lumen_engine::media::FontResolver for NoFont {
    fn id(&self) -> &str {
        "no-font"
    }
    fn data(&self) -> Result<Vec<Vec<u8>>, lumen_engine::error::MediaError> {
        Ok(Vec::new())
    }
}

impl MediaStore for LateFont {
    fn get_image_resolver(&self, _: &str) -> Option<Box<dyn ImageResolver>> {
        None
    }
    fn get_video_resolver(&self, _: &str) -> Option<Box<dyn VideoFrameResolver>> {
        None
    }
    fn font_resolver_id(&self, family: &str) -> Option<String> {
        match family {
            "Loader" => Some("late-font".to_string()),
            "Qzxvj" => Some("no-font".to_string()),
            _ => None,
        }
    }
    fn get_font_resolver(
        &self,
        family: &str,
    ) -> Option<Box<dyn lumen_engine::media::FontResolver>> {
        match family {
            "Loader" => Some(Box::new(LateFont)),
            "Qzxvj" => Some(Box::new(NoFont)),
            _ => None,
        }
    }
}

#[test]
fn a_font_loaded_while_binding_resizes_constant_text_instead_of_failing() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    // The first text measures a node whose font loads as the frame binds. That font, unlike the
    // bundled one, has no "ffi" ligature, so the second text, sized for one glyph, needs three.
    let composition = lumen_engine::json::parse_value(&json!({
        "timeline": { "fps": 24, "duration_frames": 1 },
        "render_settings": { "width": 128, "height": 64 },
        "nodes": [
            { "id": 99, "type": "text", "params": { "content": "x", "font_family": "Loader" } },
            { "id": 1, "type": "text", "params": {
                "content": "a",
                "glyph_offset": "=vec2(0 * text_width(node(99)), 0)"
            } },
            { "id": 2, "type": "text", "params": {
                "content": "ffi", "font_family": "Qzxvj", "font_size": 40, "position": [4, 10]
            } },
            { "id": 3, "type": "raster_multimerge" },
            { "id": 4, "type": "media_output" }
        ],
        "connections": [
            { "from_node": 1, "from_port": "output", "to_node": 3, "to_port": "layers" },
            { "from_node": 2, "from_port": "output", "to_node": 3, "to_port": "layers" },
            { "from_node": 3, "to_node": 4, "to_port": "source" }
        ]
    }))
    .unwrap();
    let pixels = render_rgba8(&mut renderer, &composition, 0, &LateFont);
    assert!(pixels.as_chunks::<4>().0.iter().any(|pixel| pixel[3] > 0));
}

/// Resolves "LoadN" to a font named "LateN" and "LateN" to no font data, for each digit N.
#[derive(Debug, Clone)]
struct LateFonts;

#[derive(Debug, Clone)]
struct LateFontsResolver(String);

impl lumen_engine::media::FontResolver for LateFontsResolver {
    fn id(&self) -> &str {
        &self.0
    }
    fn data(&self) -> Result<Vec<Vec<u8>>, lumen_engine::error::MediaError> {
        Ok(match self.0.strip_prefix("Load") {
            Some(digit) => vec![renamed_inter(&format!("Late{digit}"))],
            None => Vec::new(),
        })
    }
}

impl MediaStore for LateFonts {
    fn get_image_resolver(&self, _: &str) -> Option<Box<dyn ImageResolver>> {
        None
    }
    fn get_video_resolver(&self, _: &str) -> Option<Box<dyn VideoFrameResolver>> {
        None
    }
    fn font_resolver_id(&self, family: &str) -> Option<String> {
        (family.starts_with("Load") || family.starts_with("Late")).then(|| family.to_string())
    }
    fn get_font_resolver(
        &self,
        family: &str,
    ) -> Option<Box<dyn lumen_engine::media::FontResolver>> {
        self.font_resolver_id(family)
            .map(|id| Box::new(LateFontsResolver(id)) as Box<dyn lumen_engine::media::FontResolver>)
    }
}

#[test]
fn many_fonts_loaded_while_binding_settle_in_one_render() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    // Each pair is a text measuring a node whose font loads as the frame binds, then a text that
    // font outgrows, so every font stops a bind.
    let mut nodes = vec![
        json!({ "id": 100, "type": "raster_multimerge" }),
        json!({ "id": 101, "type": "media_output" }),
    ];
    let mut connections = vec![json!({ "from_node": 100, "to_node": 101, "to_port": "source" })];
    for font in 0..5 {
        let (measured, measuring, outgrown) = (200 + font, 2 * font + 1, 2 * font + 2);
        nodes.extend([
            json!({ "id": measured, "type": "text", "params": {
                "content": "x", "font_family": format!("Load{font}")
            } }),
            json!({ "id": measuring, "type": "text", "params": {
                "content": "a",
                "glyph_offset": format!("=vec2(0 * text_width(node({measured})), 0)")
            } }),
            json!({ "id": outgrown, "type": "text", "params": {
                "content": "ffi", "font_family": format!("Late{font}"), "font_size": 40
            } }),
        ]);
        for from_node in [measuring, outgrown] {
            connections.push(json!({
                "from_node": from_node, "from_port": "output", "to_node": 100, "to_port": "layers"
            }));
        }
    }
    let composition = lumen_engine::json::parse_value(&json!({
        "timeline": { "fps": 24, "duration_frames": 1 },
        "render_settings": { "width": 128, "height": 64 },
        "nodes": nodes,
        "connections": connections
    }))
    .unwrap();
    let pixels = render_rgba8(&mut renderer, &composition, 0, &LateFonts);
    assert!(pixels.as_chunks::<4>().0.iter().any(|pixel| pixel[3] > 0));
}

/// Gives "Churn" a resolver with a new id each time it is asked, so its font never settles.
#[derive(Debug, Default)]
struct ChurningFont(std::sync::atomic::AtomicU32);

#[derive(Debug, Clone)]
struct ChurningFontResolver(String);

impl lumen_engine::media::FontResolver for ChurningFontResolver {
    fn id(&self) -> &str {
        &self.0
    }
    fn data(&self) -> Result<Vec<Vec<u8>>, lumen_engine::error::MediaError> {
        Ok(vec![renamed_inter("Churn")])
    }
}

impl MediaStore for ChurningFont {
    fn get_image_resolver(&self, _: &str) -> Option<Box<dyn ImageResolver>> {
        None
    }
    fn get_video_resolver(&self, _: &str) -> Option<Box<dyn VideoFrameResolver>> {
        None
    }
    fn font_resolver_id(&self, family: &str) -> Option<String> {
        (family == "Churn").then(|| {
            let id = self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            format!("churn-{id}")
        })
    }
    fn get_font_resolver(
        &self,
        family: &str,
    ) -> Option<Box<dyn lumen_engine::media::FontResolver>> {
        self.font_resolver_id(family).map(|id| {
            Box::new(ChurningFontResolver(id)) as Box<dyn lumen_engine::media::FontResolver>
        })
    }
}

#[test]
fn a_font_that_never_settles_fails_the_render() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let composition = lumen_engine::json::parse_value(&json!({
        "timeline": { "fps": 24, "duration_frames": 1 },
        "render_settings": { "width": 64, "height": 64 },
        "nodes": [
            { "id": 1, "type": "text", "params": { "content": "x", "font_family": "Churn" } },
            { "id": 2, "type": "media_output" }
        ],
        "connections": [{ "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" }]
    }))
    .unwrap();
    let error = renderer
        .render_frame(&composition, 0, &ChurningFont::default())
        .expect_err("the font loads again on every bind")
        .to_string();
    assert!(error.contains("fonts kept loading"), "{error}");
}

#[test]
fn text_dropped_by_a_full_atlas_still_counts_toward_the_instance_limit() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    // Glyphs this large overflow the atlas, which drops some of them.
    let mut composition = text_test_composition(TextParamsDelegate {
        content: TextContentDelegate::Text(Deferred::value(
            "ABCDEFGHIJKLMNOPQRSTUVWXYZ".repeat(80),
        )),
        font_size: expression("1500 + frame * 0"),
        render_mode: lumen_engine::node::source::text::TextRenderMode::Raster.into(),
        ..Default::default()
    })
    .into_builder();
    composition.render_settings.width = 64;
    composition.render_settings.height = 64;
    let composition = composition.build().unwrap();
    let error = renderer
        .render_frame(&composition, 0, &EmptyMediaStore)
        .expect_err("2080 glyphs exceed the limit")
        .to_string();
    assert!(
        error.contains("Text node 1") && error.contains("2080") && error.contains("2048"),
        "{error}"
    );
}

fn expression<T>(source: &str) -> Deferred<T> {
    Deferred::Expr(Expression::parse(source).unwrap())
}

fn wgsl_shader_node(id: NodeId, shader: Deferred<String>, sources: Vec<PortRef>) -> NodeKind {
    NodeKind::WgslShader(WgslShader {
        id,
        params: WgslShaderParamsDelegate {
            shader,
            uniforms: Deferred::value(ShaderUniforms::default()),
        },
        sources,
    })
}

const CACHE_CANVAS: (u32, u32) = (32, 16);

fn canvas_background(id: NodeId, color: [u8; 4]) -> NodeKind {
    NodeKind::Background(Background {
        id,
        params: BackgroundParamsDelegate {
            paint: Paint::solid(color).into(),
            width: Deferred::value(CACHE_CANVAS.0),
            height: Deferred::value(CACHE_CANVAS.1),
            paint_supersample: Deferred::value(true),
        },
    })
}

fn square(id: NodeId, position: Deferred<(f64, f64)>, size: i64, color: [u8; 4]) -> NodeKind {
    NodeKind::Shape(Shape {
        id,
        params: ShapeParamsDelegate {
            position,
            width: Deferred::value(size),
            height: Deferred::value(size),
            fill_paint: Paint::solid(color).into(),
            ..Default::default()
        },
    })
}

fn multimerge(id: NodeId, layers: &[u64]) -> NodeKind {
    NodeKind::RasterMultiMerge(RasterMultiMerge {
        id,
        layers: layers
            .iter()
            .map(|layer| PortRef::new(NodeId::new(*layer), "output".to_string()))
            .collect(),
        ..Default::default()
    })
}

fn output_node(id: NodeId, source: u64) -> NodeKind {
    NodeKind::MediaOutput(MediaOutput {
        id,
        source: PortRef::new(NodeId::new(source), "output".to_string()),
    })
}

fn cache_composition(nodes: Vec<NodeKind>) -> Composition {
    let mut graph = Graph::new();
    for node in nodes {
        graph.nodes.insert(node.id(), node);
    }
    let mut composition = test_composition(graph).into_builder();
    composition.render_settings.width = CACHE_CANVAS.0;
    composition.render_settings.height = CACHE_CANVAS.1;
    let composition = composition.build().unwrap();
    composition
}

const MEMO_FRAME_SHADER: &str = "
@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let color = vec4<f32>(f32(lumen.frame) / 255.0, lumen.time, 0.5, 1.0);
    textureStore(output, vec2<i32>(id.xy), color);
}
";

/// A frame-dependent shader, read through a memo keyed by `key` when there is one.
fn memo_shader_composition(key: Option<&str>) -> Composition {
    let (shader, memo, output, input) = (
        NodeId::new(1),
        NodeId::new(2),
        NodeId::new(3),
        NodeId::new(4),
    );
    let mut nodes = vec![
        canvas_background(input, [0, 0, 0, 255]),
        wgsl_shader_node(
            shader,
            Deferred::value(MEMO_FRAME_SHADER.to_string()),
            vec![PortRef::new(input, "output".to_string())],
        ),
        output_node(output, if key.is_some() { 2 } else { 1 }),
    ];
    if let Some(key) = key {
        nodes.push(NodeKind::Memo(Memo {
            id: memo,
            params: MemoParamsDelegate {
                key: expression(key),
            },
            source: PortRef::new(shader, "output".to_string()),
        }));
    }
    cache_composition(nodes)
}

#[test]
fn resize_reads_crop_bounds_through_opacity() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let composition = |through_opacity: bool| {
        let mut nodes = vec![
            canvas_background(NodeId::new(1), [255, 0, 0, 255]),
            square(
                NodeId::new(2),
                Deferred::value((6.0, 6.0)),
                6,
                [20, 40, 220, 255],
            ),
            multimerge(NodeId::new(3), &[1, 2]),
            NodeKind::Crop(Crop {
                id: NodeId::new(4),
                params: CropParamsDelegate {
                    x: Deferred::value(4),
                    width: Deferred::value(8),
                    height: Deferred::value(8),
                    ..Default::default()
                },
                source: PortRef::new(NodeId::new(3), "output".to_string()),
            }),
        ];
        let mut last = 4;
        if through_opacity {
            nodes.push(NodeKind::Opacity(Opacity {
                id: NodeId::new(5),
                source: PortRef::new(NodeId::new(4), "output".to_string()),
                ..Default::default()
            }));
            last = 5;
        }
        nodes.push(NodeKind::Resize(Resize {
            id: NodeId::new(6),
            params: ResizeParamsDelegate {
                width: Deferred::value(i64::from(CACHE_CANVAS.0)),
                height: Deferred::value(i64::from(CACHE_CANVAS.1)),
                mode: ResizeMode::Fit.into(),
                sampling: ResizeSampling::Nearest.into(),
                ..Default::default()
            },
            source: PortRef::new(NodeId::new(last), "output".to_string()),
        }));
        nodes.push(output_node(NodeId::new(7), 6));
        cache_composition(nodes)
    };
    let direct = render_rgba8(&mut renderer, &composition(false), 0, &EmptyMediaStore);
    let through_opacity = render_rgba8(&mut renderer, &composition(true), 0, &EmptyMediaStore);
    assert!(
        direct == through_opacity,
        "opacity dropped the crop's bounds"
    );
}

fn render_rgba8_at(
    renderer: &mut lumen_engine::gpu::GpuCompositionRenderer,
    composition: &Composition,
    frame: f64,
) -> Vec<u8> {
    let raster = renderer
        .render_frame_at(composition, frame, &EmptyMediaStore)
        .unwrap();
    read_rgba8(renderer.gpu_renderer(), raster)
}

#[test]
fn plan_cache_budget_keeps_the_plan_a_bound_frame_copies_from() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let composition = memo_shader_composition(Some("frame * 0"));
    let first = render_rgba8_at(&mut renderer, &composition, 0.0);
    let bound = renderer
        .bind_frame_at(&composition, 0.5, &EmptyMediaStore)
        .unwrap();
    renderer.set_plan_cache_budget(0);
    let raster = renderer.submit_bound_frame(&bound).unwrap().0;
    assert!(read_rgba8(renderer.gpu_renderer(), raster) == first);
}

fn solid_image(id: &str, width: u32, height: u32, color: [u8; 4]) -> TestMediaStore {
    TestMediaStore {
        id: id.to_string(),
        frame: Arc::new(CpuMediaFrame {
            rgba: Arc::new(color.repeat(width as usize * height as usize)),
            width,
            height,
            row_bytes: width as usize * 4,
        }),
    }
}

/// Images by source, which their owner replaces while they are rendered.
#[derive(Debug, Default)]
struct ReplaceableImages(std::sync::Mutex<std::collections::HashMap<String, TestMediaStore>>);

impl ReplaceableImages {
    fn set(&self, source: &str, image: TestMediaStore) {
        self.0.lock().unwrap().insert(source.to_string(), image);
    }
}

impl MediaStore for ReplaceableImages {
    fn get_image_resolver(&self, source: &str) -> Option<Box<dyn ImageResolver>> {
        let images = self.0.lock().unwrap();
        let image = images.get(source)?.clone();
        Some(Box::new(image))
    }

    fn get_video_resolver(&self, _stream_id: &str) -> Option<Box<dyn VideoFrameResolver>> {
        None
    }
}

fn image_composition(source: Deferred<String>) -> Composition {
    cache_composition(vec![
        NodeKind::MediaIn(MediaIn {
            id: NodeId::new(1),
            params: MediaInParamsDelegate {
                source,
                ..Default::default()
            },
        }),
        output_node(NodeId::new(2), 1),
    ])
}

/// The RGBA of the pixel at `(x, y)` of a `CACHE_CANVAS`-sized render.
fn canvas_pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let start = (y * CACHE_CANVAS.0 + x) as usize * 4;
    pixels[start..start + 4].try_into().unwrap()
}

#[test]
fn canvas_sized_media_passthrough_writes_external_targets() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let color = [40, 120, 200, 255];
    let media = solid_image("plate", CACHE_CANVAS.0, CACHE_CANVAS.1, color);
    for kind in [0, 1] {
        let composition = cache_composition(vec![
            NodeKind::MediaIn(MediaIn {
                id: NodeId::new(1),
                params: MediaInParamsDelegate {
                    kind: Deferred::value(kind),
                    source: Deferred::value("plate".to_string()),
                    ..Default::default()
                },
            }),
            output_node(NodeId::new(2), 1),
        ]);
        render_rgba8(&mut renderer, &composition, 0, &media);
        let output = renderer.compiled().unwrap().output;
        let gpu = renderer.gpu_renderer();
        let desc = gpu.texture_desc(output.texture).unwrap();
        let texture = gpu
            .device
            .create_texture(&lumen_gpu::wgpu::TextureDescriptor {
                label: Some("external output"),
                size: desc.domain.storage_size.as_extent(),
                mip_level_count: 1,
                sample_count: 1,
                dimension: lumen_gpu::wgpu::TextureDimension::D2,
                format: desc.format,
                usage: desc.usage | lumen_gpu::wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
        let submitted = renderer
            .render_frame_into_external(
                &composition,
                0,
                &media,
                lumen_gpu::ExternalTexture::from_texture(texture, desc.domain),
            )
            .unwrap();
        submitted.wait(&renderer.gpu_renderer().device).unwrap();
        let pixels = read_texture_rgba8(
            renderer.gpu_renderer(),
            submitted.texture(),
            desc.domain.storage_size,
        );
        assert!(
            pixels == color.repeat((CACHE_CANVAS.0 * CACHE_CANVAS.1) as usize),
            "media kind {kind} left its external target at {:?}",
            canvas_pixel(&pixels, 0, 0)
        );
    }
}

#[test]
fn media_of_another_size_renders_at_its_own_size() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let red = [255, 0, 0, 255];
    let blue = [0, 0, 255, 255];
    let (width, height) = CACHE_CANVAS;
    let inside = (1, 1);
    let outside = (width - 1, height - 1);

    // A source expression that picks an image of another size.
    let media = ReplaceableImages::default();
    media.set("large", solid_image("large", width, height, red));
    media.set("small", solid_image("small", 2, 2, blue));
    let chosen = image_composition(expression("if(frame < 1, 'large', 'small')"));
    let pixels = render_rgba8(&mut renderer, &chosen, 0, &media);
    assert_eq!(canvas_pixel(&pixels, outside.0, outside.1), red);
    let pixels = render_rgba8(&mut renderer, &chosen, 1, &media);
    assert_eq!(canvas_pixel(&pixels, inside.0, inside.1), blue);
    assert_eq!(canvas_pixel(&pixels, outside.0, outside.1), [0; 4]);

    // The same source replaced by an image of another size.
    renderer.release_resources();
    media.set("plate", solid_image("plate", width, height, red));
    let replaced = image_composition(Deferred::value("plate".to_string()));
    let pixels = render_rgba8(&mut renderer, &replaced, 0, &media);
    assert_eq!(canvas_pixel(&pixels, outside.0, outside.1), red);
    media.set("plate", solid_image("plate", 2, 2, blue));
    let pixels = render_rgba8(&mut renderer, &replaced, 1, &media);
    assert_eq!(canvas_pixel(&pixels, inside.0, inside.1), blue);
    assert_eq!(canvas_pixel(&pixels, outside.0, outside.1), [0; 4]);
}

#[test]
fn replacing_an_image_with_one_of_the_same_size_shows_its_pixels() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let (width, height) = CACHE_CANVAS;
    let red = [255, 0, 0, 255];
    let blue = [0, 0, 255, 255];
    let media = ReplaceableImages::default();
    media.set("plate", solid_image("plate#1", width, height, red));
    let composition = image_composition(Deferred::value("plate".to_string()));
    let pixels = render_rgba8(&mut renderer, &composition, 0, &media);
    assert_eq!(canvas_pixel(&pixels, 1, 1), red);

    media.set("plate", solid_image("plate#2", width, height, blue));
    let pixels = render_rgba8(&mut renderer, &composition, 1, &media);
    assert_eq!(canvas_pixel(&pixels, 1, 1), blue);

    media.set("plate", solid_image("plate#3", width, height, red));
    let pixels = render_rgba8(&mut renderer, &composition, 2, &media);
    assert_eq!(canvas_pixel(&pixels, 1, 1), red);
}

#[test]
fn unbound_images_within_the_budget_stay_cached() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let (width, height) = CACHE_CANVAS;
    let colors = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]];
    let images = colors.map(|color| solid_image(&format!("{color:?}"), width, height, color));
    // The two images no plan binds fit exactly; the bound one doesn't count.
    renderer.set_media_texture_cache_budget(2 * u64::from(width * height * 4));
    let media = ReplaceableImages::default();
    let composition = image_composition(Deferred::value("plate".to_string()));
    for frame in 0..9 {
        media.set("plate", images[frame % 3].clone());
        let pixels = render_rgba8(&mut renderer, &composition, frame as u32, &media);
        assert_eq!(canvas_pixel(&pixels, 1, 1), colors[frame % 3]);
    }
    assert_eq!(renderer.media_texture_upload_count(), 3);
    assert_eq!(renderer.cached_media_texture_count(), 3);
}

#[test]
fn images_past_the_budget_are_freed_once_no_plan_binds_them() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let (width, height) = CACHE_CANVAS;
    renderer.set_media_texture_cache_budget(0);
    let media = ReplaceableImages::default();
    let composition = image_composition(Deferred::value("plate".to_string()));
    for (frame, color) in [[255, 0, 0, 255], [0, 0, 255, 255]].into_iter().enumerate() {
        media.set(
            "plate",
            solid_image(&format!("{color:?}"), width, height, color),
        );
        render_rgba8(&mut renderer, &composition, frame as u32, &media);
    }
    // Only the image the plan shows is kept.
    assert_eq!(renderer.cached_media_texture_count(), 1);
    renderer.set_static_output_reuse(false);
    assert_eq!(renderer.cached_media_texture_count(), 0);
}

#[test]
fn video_frames_do_not_overwrite_cached_images() {
    #[derive(Debug)]
    struct ImagesAndVideo {
        images: ReplaceableImages,
        video: TestMediaStore,
    }
    impl MediaStore for ImagesAndVideo {
        fn get_image_resolver(&self, source: &str) -> Option<Box<dyn ImageResolver>> {
            self.images.get_image_resolver(source)
        }

        fn get_video_resolver(&self, _stream_id: &str) -> Option<Box<dyn VideoFrameResolver>> {
            Some(Box::new(self.video.clone()))
        }
    }

    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let (width, height) = CACHE_CANVAS;
    let red = [255, 0, 0, 255];
    let blue = [0, 0, 255, 255];
    let green = [0, 255, 0, 255];
    let yellow = [255, 255, 0, 255];
    let media = ImagesAndVideo {
        images: ReplaceableImages::default(),
        video: solid_image("clip", width, height, blue),
    };
    media.images.set("a", solid_image("a", width, height, red));
    media
        .images
        .set("b", solid_image("b", width, height, green));
    media
        .images
        .set("c", solid_image("c", width, height, yellow));
    let composition = cache_composition(vec![
        NodeKind::MediaIn(MediaIn {
            id: NodeId::new(1),
            params: MediaInParamsDelegate {
                kind: expression("if(frame == 1, 1, 0)"),
                source: expression(
                    "if(frame == 1, 'clip', if(frame == 2, 'b', if(frame == 3, 'c', 'a')))",
                ),
                ..Default::default()
            },
        }),
        output_node(NodeId::new(2), 1),
    ]);
    for (frame, expected) in [red, blue, green, yellow, red].into_iter().enumerate() {
        let pixels = render_rgba8(&mut renderer, &composition, frame as u32, &media);
        assert_eq!(canvas_pixel(&pixels, 1, 1), expected, "frame {frame}");
    }
}

#[test]
fn path_points_buffer_is_sized_to_the_flattened_path() {
    let polyline = |count: usize| {
        let mut data = String::from("M 10 10");
        for index in 1..count - 2 {
            let x = 10.0 + 100.0 * index as f64 / (count - 3) as f64;
            data += &format!(" L {x:.4} {}", if index % 2 == 0 { 10.0 } else { 11.0 });
        }
        data + " L 110 90 L 10 90 Z"
    };
    let path = |data: String| {
        NodeKind::Path(Path {
            id: NodeId::new(1),
            params: PathParamsDelegate {
                data: Deferred::value(data),
                ..Default::default()
            },
        })
    };

    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    render_drawable_graph(&mut renderer, vec![path(polyline(400))], 128);
    let points_buffer = renderer
        .compiled()
        .unwrap()
        .plan
        .buffers()
        .iter()
        .find(|buffer| buffer.label.as_deref() == Some("path:1:points"))
        .unwrap()
        .desc
        .size;
    assert_eq!(points_buffer, 400 * 24, "sized to the flattened path");
}
