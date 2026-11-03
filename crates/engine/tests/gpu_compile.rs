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
            paint::{GradientPaint, GradientStop, Paint, PaintKind},
            path::{Path, PathParamsDelegate},
            shape::{Shape, ShapeGeometryKind, ShapeParamsDelegate},
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

#[cfg(feature = "msdf")]
#[test]
fn text_msdf_coverage_matches_raster_for_plain_italic_and_variable_weight() {
    use lumen_engine::node::source::text::{TextFontStyle, TextRenderMode};
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    #[derive(Debug, Clone)]
    struct InterFont;
    impl lumen_engine::media::FontResolver for InterFont {
        fn id(&self) -> &str {
            "msdf-inter-coverage"
        }
        fn data(&self) -> Result<Vec<Vec<u8>>, lumen_engine::error::MediaError> {
            Ok(vec![
                include_bytes!("../../text/assets/inter/InterVariable-Latin.ttf").to_vec(),
            ])
        }
    }
    impl MediaStore for InterFont {
        fn get_image_resolver(&self, _: &str) -> Option<Box<dyn ImageResolver>> {
            None
        }
        fn get_video_resolver(&self, _: &str) -> Option<Box<dyn VideoFrameResolver>> {
            None
        }
        fn font_resolver_id(&self, family: &str) -> Option<String> {
            (family == "Inter").then(|| "msdf-inter-coverage".to_string())
        }
        fn get_font_resolver(
            &self,
            family: &str,
        ) -> Option<Box<dyn lumen_engine::media::FontResolver>> {
            (family == "Inter")
                .then(|| Box::new(self.clone()) as Box<dyn lumen_engine::media::FontResolver>)
        }
    }
    for (family, style, weight) in [
        ("Roboto", TextFontStyle::Normal, 400),
        ("Roboto", TextFontStyle::Italic, 400),
        ("Inter", TextFontStyle::Normal, 250),
        ("Inter", TextFontStyle::Normal, 850),
    ] {
        for size in [24.0, 72.0, 144.0] {
            let composition = text_test_composition(TextParamsDelegate {
                content: TextContentDelegate::Text(Deferred::value("AV text 28".to_string())),
                font_family: Deferred::value(family.to_string()),
                font_style: style.into(),
                font_weight: Deferred::value(weight),
                font_size: Deferred::value(size),
                position: Deferred::value((12.25, 8.5)),
                ..Default::default()
            });
            renderer
                .compile_with_media(
                    &composition,
                    &InterFont,
                    lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                )
                .unwrap();
            let raster = render_rgba8(&mut renderer, &composition, 0, &InterFont);
            let mut composition = composition.into_builder();
            let NodeKind::Text(text) = composition.graph.nodes.get_mut(&NodeId::new(1)).unwrap()
            else {
                unreachable!()
            };
            text.params.render_mode = TextRenderMode::Msdf.into();
            let composition = composition.build().unwrap();

            renderer
                .compile_with_media(
                    &composition,
                    &InterFont,
                    lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                )
                .unwrap();
            let bound = renderer.bind_frame(&composition, 0, &InterFont).unwrap();
            let instances = renderer
                .compiled()
                .unwrap()
                .plan
                .buffers()
                .iter()
                .find(|b| b.label.as_deref() == Some("text:1:instances"))
                .unwrap()
                .id;
            let has_fields = bound.frame_update().uploads().iter().any(|upload| {
                let lumen_gpu::Upload::Buffer { id, data, .. } = upload else {
                    return false;
                };
                *id == instances
                    && data
                        .as_chunks::<{ std::mem::size_of::<lumen_text::GpuGlyphInstance>() }>()
                        .0
                        .iter()
                        .any(|bytes| {
                            bytemuck::pod_read_unaligned::<lumen_text::GpuGlyphInstance>(bytes)
                                .render_mode()
                                == lumen_text::GLYPH_MODE_MSDF
                        })
            });
            assert!(
                has_fields,
                "{family} {style:?} {weight} {size}: no MSDF glyphs"
            );
            let msdf = render_rgba8(&mut renderer, &composition, 0, &InterFont);
            let (intersection, union) = raster
                .as_chunks::<4>()
                .0
                .iter()
                .zip(msdf.as_chunks::<4>().0.iter())
                .fold((0u64, 0u64), |(i, u), (a, b)| {
                    (i + u64::from(a[3].min(b[3])), u + u64::from(a[3].max(b[3])))
                });
            let iou = intersection as f64 / union as f64;
            eprintln!("{family} {style:?} weight={weight} size={size}: IoU={iou:.3}");
            let minimum = if size == 24.0 { 0.68 } else { 0.90 };
            assert!(
                iou > minimum,
                "{family} {style:?} weight={weight} size={size}: coverage IoU={iou:.3}"
            );
        }
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
fn text_transparent_gradient_midpoint_is_premultiplied() {
    use lumen_engine::node::{
        source::text::TextRenderMode,
        vector::paint::{GradientPaint, GradientStop, GradientUnits},
    };
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    for mode in [TextRenderMode::Raster, TextRenderMode::Msdf] {
        for supersample in [false, true] {
            let composition = text_test_composition(TextParamsDelegate {
                content: TextContentDelegate::Text(Deferred::value("M".to_string())),
                font_size: Deferred::value(180.0),
                render_mode: mode.into(),
                paint_supersample: Deferred::value(supersample),
                color: Paint::Gradient(GradientPaint {
                    units: GradientUnits::UserSpace,
                    start: [-1000000.0, 0.0],
                    end: [1000000.0, 0.0],
                    stops: vec![
                        GradientStop {
                            offset: 0.0,
                            color: [255, 0, 0, 0],
                        },
                        GradientStop {
                            offset: 1.0,
                            color: [0, 0, 255, 255],
                        },
                    ],
                    ..Default::default()
                })
                .into(),
                ..Default::default()
            });
            renderer.release_resources();
            let pixels = render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);
            assert!(
                pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| pixel[3] >= 126)
            );
            for pixel in pixels
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|pixel| pixel[3] >= 126)
            {
                assert!(
                    pixel[0] <= 1
                        && pixel[1] <= 1
                        && pixel[2].abs_diff(128) <= 2
                        && pixel[3].abs_diff(128) <= 2,
                    "{mode:?}, supersample={supersample}: {pixel:?}"
                );
            }
        }
    }
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

fn max_channel_difference(a: &[u8], b: &[u8]) -> u8 {
    a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)).max().unwrap()
}

#[test]
fn drawable_opacity_scales_premultiplied_output() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let gradient = Paint::Gradient(GradientPaint {
        kind: PaintKind::LinearGradient,
        stops: vec![
            GradientStop {
                offset: 0.0,
                color: [255, 0, 0, 255],
            },
            GradientStop {
                offset: 1.0,
                color: [0, 0, 255, 128],
            },
        ],
        ..Default::default()
    });
    let nodes = |opacity: f64| {
        [
            NodeKind::Shape(Shape {
                id: NodeId::new(1),
                params: ShapeParamsDelegate {
                    geometry_kind: ShapeGeometryKind::Ellipse.into(),
                    width: Deferred::value(40),
                    height: Deferred::value(30),
                    position: Deferred::value((4.5, 6.0)),
                    fill_paint: gradient.clone().into(),
                    opacity: Deferred::value(opacity),
                    ..Default::default()
                },
            }),
            NodeKind::Path(Path {
                id: NodeId::new(1),
                params: PathParamsDelegate {
                    data: Deferred::value("M 5 5 Q 40 0 45 40 L 5 45 Z".to_string()),
                    fill_paint: gradient.clone().into(),
                    stroke_enabled: Deferred::value(true),
                    stroke_width: Deferred::value(2.0),
                    opacity: Deferred::value(opacity),
                    ..Default::default()
                },
            }),
        ]
    };
    for (opaque, translucent) in nodes(1.0).into_iter().zip(nodes(0.5)) {
        let opaque = render_drawable_graph(&mut renderer, vec![opaque], 50);
        let translucent = render_drawable_graph(&mut renderer, vec![translucent], 50);
        assert!(opaque.iter().skip(3).step_by(4).any(|&a| a > 200));
        for (full, half) in opaque.iter().zip(&translucent) {
            assert!(
                (f32::from(*full) * 0.5 - f32::from(*half)).abs() <= 1.0,
                "{full} at half opacity gave {half}"
            );
        }
    }
}

#[test]
fn text_transform_in_msdf_mode_matches_larger_and_rotated_text() {
    use lumen_engine::node::source::text::TextRenderMode;
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let text = |font_size: f64, scale: f64, rotation: f64, anchor: (f64, f64)| {
        vec![NodeKind::Text(Text {
            id: NodeId::new(1),
            params: TextParamsDelegate {
                content: TextContentDelegate::Text(Deferred::value("Lumen AV".to_string())),
                font_size: Deferred::value(font_size),
                render_mode: TextRenderMode::Msdf.into(),
                position: Deferred::value((20.0, 30.0)),
                scale: Deferred::value((scale, scale)),
                rotation: Deferred::value(rotation),
                anchor: Deferred::value(anchor),
                ..Default::default()
            },
        })]
    };
    let size = 256;
    let coverage =
        |pixels: &[u8]| -> Vec<u8> { pixels.iter().skip(3).step_by(4).copied().collect() };
    let iou = |a: &[u8], b: &[u8]| {
        let (intersection, union) = a.iter().zip(b).fold((0u64, 0u64), |(i, u), (a, b)| {
            (i + u64::from(*a.min(b)), u + u64::from(*a.max(b)))
        });
        intersection as f64 / union as f64
    };

    // Scaled glyphs keep MSDF edges as sharp as glyphs laid out at the larger size.
    let scaled = coverage(&render_drawable_graph(
        &mut renderer,
        text(16.0, 3.0, 0.0, (0.0, 0.0)),
        size,
    ));
    let larger = coverage(&render_drawable_graph(
        &mut renderer,
        text(48.0, 1.0, 0.0, (0.0, 0.0)),
        size,
    ));
    let scaled_edges = scaled.iter().filter(|&&a| a > 0 && a < 255).count();
    let larger_edges = larger.iter().filter(|&&a| a > 0 && a < 255).count();
    eprintln!(
        "msdf scale: IoU {:.3}, edges {scaled_edges} vs {larger_edges}",
        iou(&scaled, &larger)
    );
    assert!(iou(&scaled, &larger) > 0.9);
    assert!(scaled_edges.abs_diff(larger_edges) * 4 < larger_edges);

    // A quarter turn about a whole-pixel anchor moves pixel centres onto pixel centres.
    let upright = coverage(&render_drawable_graph(
        &mut renderer,
        text(24.0, 2.0, 0.0, (40.0, 20.0)),
        size,
    ));
    let turned = coverage(&render_drawable_graph(
        &mut renderer,
        text(24.0, 2.0, 90.0, (40.0, 20.0)),
        size,
    ));
    let (pivot_x, pivot_y) = (60i64, 50i64);
    let mut expected = vec![0u8; turned.len()];
    for y in 0..size as i64 {
        for x in 0..size as i64 {
            let (tx, ty) = (pivot_x - (y - pivot_y) - 1, pivot_y + (x - pivot_x));
            if (0..size as i64).contains(&tx) && (0..size as i64).contains(&ty) {
                expected[(ty * size as i64 + tx) as usize] =
                    upright[(y * size as i64 + x) as usize];
            }
        }
    }
    let turned_iou = iou(&turned, &expected);
    eprintln!("msdf rotation: IoU {turned_iou:.3}");
    assert!(upright.iter().any(|&a| a == 255));
    assert!(turned_iou > 0.97);
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
fn text_opacity_fades_overlapping_glyphs_as_one_layer() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let text = |opacity: f64| {
        vec![NodeKind::Text(Text {
            id: NodeId::new(1),
            params: TextParamsDelegate {
                content: TextContentDelegate::Text(Deferred::value(
                    "A\u{338} f\u{301}".to_string(),
                )),
                font_size: Deferred::value(48.0),
                position: Deferred::value((8.0, 8.0)),
                opacity: Deferred::value(opacity),
                ..Default::default()
            },
        })]
    };
    let opaque = render_drawable_graph(&mut renderer, text(1.0), 128);
    let translucent = render_drawable_graph(&mut renderer, text(0.5), 128);
    assert!(opaque.iter().skip(3).step_by(4).any(|&a| a == 255));
    for (full, half) in opaque.iter().zip(&translucent) {
        assert!(
            (f32::from(*full) * 0.5 - f32::from(*half)).abs() <= 1.0,
            "{full} at half opacity gave {half}"
        );
    }
}

#[test]
fn scaled_text_supersamples_paint_over_one_canvas_pixel() {
    use lumen_engine::node::{source::text::TextRenderMode, vector::paint::GradientUnits};
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let stop = |offset: f32, color: [u8; 4]| GradientStop { offset, color };
    let red = [255, 0, 0, 255];
    let blue = [0, 0, 255, 255];
    // A hard red-to-blue stop through the crossbar of an `H` scaled 8x, at canvas x 57.6.
    let text = vec![NodeKind::Text(Text {
        id: NodeId::new(1),
        params: TextParamsDelegate {
            content: TextContentDelegate::Text(Deferred::value("H".to_string())),
            font_size: Deferred::value(16.0),
            render_mode: TextRenderMode::Msdf.into(),
            position: Deferred::value((8.0, 8.0)),
            scale: Deferred::value((8.0, 8.0)),
            paint_supersample: Deferred::value(true),
            color: Paint::Gradient(GradientPaint {
                units: GradientUnits::UserSpace,
                start: [0.0, 0.0],
                end: [28.4, 0.0],
                stops: vec![
                    stop(0.0, red),
                    stop(0.5, red),
                    stop(0.5, blue),
                    stop(1.0, blue),
                ],
                ..Default::default()
            })
            .into(),
            ..Default::default()
        },
    })];
    let pixels = render_drawable_graph(&mut renderer, text, 128);
    let mixed_columns = (0..128)
        .filter(|&x| {
            (0..128).any(|y| {
                let pixel = &pixels[(y * 128 + x) * 4..][..4];
                pixel[3] == 255 && pixel[0] > 8 && pixel[2] > 8
            })
        })
        .count();
    assert!(
        pixels.as_chunks::<4>().0.iter().any(|pixel| *pixel == red)
            && pixels.as_chunks::<4>().0.iter().any(|pixel| *pixel == blue)
    );
    assert!(
        mixed_columns <= 2,
        "the stop is blurred over {mixed_columns} columns"
    );
}

#[test]
fn text_spacing_and_variable_weight_change_output_smoothly() {
    use lumen_engine::node::source::text::TextRenderMode;
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    #[derive(Debug, Clone)]
    struct InterFont;
    impl lumen_engine::media::FontResolver for InterFont {
        fn id(&self) -> &str {
            "msdf-inter-coverage"
        }
        fn data(&self) -> Result<Vec<Vec<u8>>, lumen_engine::error::MediaError> {
            Ok(vec![
                include_bytes!("../../text/assets/inter/InterVariable-Latin.ttf").to_vec(),
            ])
        }
    }
    impl MediaStore for InterFont {
        fn get_image_resolver(&self, _: &str) -> Option<Box<dyn ImageResolver>> {
            None
        }
        fn get_video_resolver(&self, _: &str) -> Option<Box<dyn VideoFrameResolver>> {
            None
        }
        fn font_resolver_id(&self, family: &str) -> Option<String> {
            (family == "Inter").then(|| "msdf-inter-coverage".to_string())
        }
        fn get_font_resolver(
            &self,
            family: &str,
        ) -> Option<Box<dyn lumen_engine::media::FontResolver>> {
            (family == "Inter")
                .then(|| Box::new(self.clone()) as Box<dyn lumen_engine::media::FontResolver>)
        }
    }
    let base = || TextParamsDelegate {
        content: TextContentDelegate::Text(Deferred::value("Hi\nHo".to_string())),
        font_family: Deferred::value("Inter".to_string()),
        font_size: Deferred::value(40.0),
        position: Deferred::value((20.0, 10.0)),
        ..Default::default()
    };
    let mut render = |params: TextParamsDelegate, frame: u32| {
        let composition = text_test_composition(params);
        renderer
            .compile_with_media(
                &composition,
                &InterFont,
                lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            )
            .unwrap();
        render_rgba8(&mut renderer, &composition, frame, &InterFont)
    };
    let width = 1024_usize;
    // Rightmost and bottommost inked pixel as (x, y), and total ink.
    let extent = |pixels: &[u8]| {
        let (mut right, mut bottom, mut ink) = (0, 0, 0_u64);
        for (index, pixel) in pixels.as_chunks::<4>().0.iter().enumerate() {
            if pixel[3] > 0 {
                right = right.max(index % width);
                bottom = bottom.max(index / width);
                ink += u64::from(pixel[3]);
            }
        }
        (right, bottom, ink)
    };

    for mode in [TextRenderMode::Raster, TextRenderMode::Msdf] {
        let with = |letter_spacing, line_height| TextParamsDelegate {
            render_mode: mode.into(),
            letter_spacing: Deferred::value(letter_spacing),
            line_height: Deferred::value(line_height),
            ..base()
        };
        let auto = render(with(0.0, 0.0), 0);
        let (auto_right, auto_bottom, _) = extent(&auto);
        assert!(auto_right > 20, "{mode:?}: nothing rendered");

        // An explicit line height equal to the automatic one changes nothing.
        let explicit = render(with(0.0, f64::from(40.0_f32 * 1.2)), 0);
        assert!(
            auto == explicit,
            "{mode:?}: explicit auto line height differs"
        );

        let (spaced_right, spaced_bottom, _) = extent(&render(with(20.0, 0.0), 0));
        assert!(
            spaced_right >= auto_right + 15,
            "{mode:?}: letter spacing did not widen ({spaced_right} vs {auto_right})"
        );
        assert_eq!(
            spaced_bottom, auto_bottom,
            "{mode:?}: letter spacing moved lines"
        );

        let (_, tall_bottom, _) = extent(&render(with(0.0, 100.0), 0));
        assert!(
            tall_bottom >= auto_bottom + 40,
            "{mode:?}: line height did not apply"
        );

        let (tight_right, _, _) = extent(&render(with(-4.0, 0.0), 0));
        assert!(
            tight_right < auto_right,
            "{mode:?}: negative spacing did not tighten"
        );
    }

    // Weights 400, 450 and 500 are distinct, and an animated weight grows the
    // glyph ink at every step.
    let at_weight = |weight| TextParamsDelegate {
        font_size: Deferred::value(96.0),
        font_weight: Deferred::value(weight),
        ..base()
    };
    let (w400, w450, w500) = (
        render(at_weight(400), 0),
        render(at_weight(450), 0),
        render(at_weight(500), 0),
    );
    assert!(w450 != w400 && w450 != w500 && w400 != w500);
    let animated = TextParamsDelegate {
        font_weight: Deferred::Expr(
            lumen_engine::expr::Expression::parse("400 + frame * 10").unwrap(),
        ),
        ..at_weight(400)
    };
    let mut previous = 0;
    for frame in 0..=10 {
        let (_, _, ink) = extent(&render(animated.clone(), frame));
        assert!(
            ink > previous,
            "frame {frame}: ink {ink} did not grow from {previous}"
        );
        previous = ink;
    }
    assert_eq!(render(animated.clone(), 5), w450);
}

#[cfg(feature = "msdf")]
#[test]
fn text_glyphs_larger_than_the_atlas_still_draw() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    // At 6000 px the stem of an "I" is taller than the 4096 px atlas.
    let mut composition = text_test_composition(TextParamsDelegate {
        content: TextContentDelegate::Text(Deferred::value("I".to_string())),
        font_size: Deferred::value(6000.0),
        position: Deferred::value((-700.0, -2000.0)),
        ..Default::default()
    })
    .into_builder();
    composition.render_settings.width = 256;
    composition.render_settings.height = 256;
    let composition = composition.build().unwrap();
    let pixels = render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);
    let inked = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| pixel[3] == 255)
        .count();
    assert!(inked > 256 * 256 / 2, "only {inked} pixels inked");
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

#[cfg(feature = "msdf")]
#[test]
fn text_msdf_corners_stay_within_a_pixel_of_raster_at_1000px() {
    use lumen_engine::node::source::text::TextRenderMode;
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let (width, height) = (1500_usize, 1100_usize);
    let mut render = |mode: TextRenderMode| {
        let mut composition = text_test_composition(TextParamsDelegate {
            content: TextContentDelegate::Text(Deferred::value("Ra".to_string())),
            font_size: Deferred::value(1000.0),
            position: Deferred::value((20.0, -180.0)),
            render_mode: mode.into(),
            ..Default::default()
        })
        .into_builder();
        composition.render_settings.width = width as u32;
        composition.render_settings.height = height as u32;
        let composition = composition.build().unwrap();
        renderer
            .compile_with_media(
                &composition,
                &EmptyMediaStore,
                lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            )
            .unwrap();
        let pixels = render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|pixel| pixel[3])
            .collect::<Vec<_>>()
    };
    let raster = render(TextRenderMode::Raster);
    let msdf = render(TextRenderMode::Msdf);
    assert!(raster.iter().filter(|alpha| **alpha == 255).count() > 50_000);

    // Fields with too few texels per em cut facets of several pixels into
    // sharp corners, so the field must stay inside the raster outline dilated
    // and eroded by one pixel.
    let mut outliers = 0;
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let (mut low, mut high) = (255_u8, 0_u8);
            for ny in y - 1..=y + 1 {
                for nx in x - 1..=x + 1 {
                    low = low.min(raster[ny * width + nx]);
                    high = high.max(raster[ny * width + nx]);
                }
            }
            let value = msdf[y * width + x];
            outliers += usize::from(low.saturating_sub(value).max(value.saturating_sub(high)) > 26);
        }
    }
    assert!(outliers <= 16, "{outliers} pixels off the raster outline");
}

fn expression<T>(source: &str) -> Deferred<T> {
    Deferred::Expr(Expression::parse(source).unwrap())
}

fn total_alpha(pixels: &[u8]) -> u64 {
    pixels
        .iter()
        .skip(3)
        .step_by(4)
        .map(|&a| u64::from(a))
        .sum()
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

fn glyph_text(content: &str, params: TextParamsDelegate) -> Vec<NodeKind> {
    vec![NodeKind::Text(Text {
        id: NodeId::new(1),
        params: TextParamsDelegate {
            content: TextContentDelegate::Text(Deferred::value(content.to_string())),
            font_size: Deferred::value(40.0),
            position: Deferred::value((20.0, 10.0)),
            ..params
        },
    })]
}

fn alpha_of(pixels: &[u8], size: u32, x: usize, y: usize) -> u8 {
    pixels[(y * size as usize + x) * 4 + 3]
}

/// Runs of columns that hold any ink, left to right.
fn inked_columns(pixels: &[u8], size: u32) -> Vec<std::ops::Range<usize>> {
    let size = size as usize;
    let inked = |x: usize| (0..size).any(|y| alpha_of(pixels, size as u32, x, y) > 0);
    let mut runs: Vec<std::ops::Range<usize>> = Vec::new();
    for x in (0..size).filter(|&x| inked(x)) {
        match runs.last_mut() {
            Some(run) if run.end == x => run.end = x + 1,
            _ => runs.push(x..x + 1),
        }
    }
    runs
}

/// `[min_x, min_y, max_x, max_y]` of the pixels in `columns` with alpha over half.
fn ink_box(pixels: &[u8], size: u32, columns: std::ops::Range<usize>) -> [usize; 4] {
    let mut bounds = [usize::MAX, usize::MAX, 0, 0];
    for y in 0..size as usize {
        for x in columns.clone() {
            if alpha_of(pixels, size, x, y) > 128 {
                bounds = [
                    bounds[0].min(x),
                    bounds[1].min(y),
                    bounds[2].max(x),
                    bounds[3].max(y),
                ];
            }
        }
    }
    bounds
}

#[test]
fn bounded_text_holds_glyphs_its_glyph_params_move() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    let text = |position: (f64, f64), glyph_offset: (f64, f64)| {
        vec![NodeKind::Text(Text {
            id: NodeId::new(1),
            params: TextParamsDelegate {
                content: TextContentDelegate::Text(Deferred::value("Hi".to_string())),
                font_size: Deferred::value(30.0),
                position: Deferred::value(position),
                glyph_offset: Deferred::value(glyph_offset),
                ..Default::default()
            },
        })]
    };
    let moved = render_drawable_graph(&mut renderer, text((10.0, 10.0), (0.0, 60.0)), size);
    let placed = render_drawable_graph(&mut renderer, text((10.0, 70.0), (0.0, 0.0)), size);
    assert!(placed.iter().skip(3).step_by(4).any(|&a| a > 0));
    assert_eq!(moved, placed);
}

#[test]
fn text_glyph_offset_and_opacity_stagger_each_glyph() {
    use lumen_engine::node::source::text::TextRenderMode;
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    for mode in [TextRenderMode::Raster, TextRenderMode::Msdf] {
        let params = || TextParamsDelegate {
            render_mode: mode.into(),
            ..Default::default()
        };
        let still = render_drawable_graph(&mut renderer, glyph_text("IIII", params()), size);
        let staggered = render_drawable_graph(
            &mut renderer,
            glyph_text(
                "IIII",
                TextParamsDelegate {
                    glyph_offset: expression("vec2(0, glyph * 10)"),
                    glyph_opacity: expression("glyph / (glyph_count - 1)"),
                    ..params()
                },
            ),
            size,
        );
        let bars = inked_columns(&still, size);
        assert_eq!(bars.len(), 4, "{mode:?}");
        // Each bar moves down 10 pixels more than the last, and fades in from the first.
        for (index, bar) in bars.into_iter().enumerate() {
            let opacity = index as f32 / 3.0;
            for y in 0..size as usize {
                for x in bar.clone() {
                    let expected = y
                        .checked_sub(10 * index)
                        .map_or(0.0, |y| f32::from(alpha_of(&still, size, x, y)) * opacity);
                    let actual = f32::from(alpha_of(&staggered, size, x, y));
                    assert!(
                        (actual - expected).abs() <= 1.5,
                        "{mode:?} glyph {index} at ({x}, {y}): {actual} vs {expected}"
                    );
                }
            }
        }
    }
}

#[test]
fn glyph_opacity_fades_a_cluster_of_overlapping_instances_as_one() {
    use lumen_engine::node::source::text::TextRenderMode;
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    for mode in [TextRenderMode::Raster, TextRenderMode::Msdf] {
        // The overlay is a second instance in the base's cluster, and they overlap.
        let render = |renderer: &mut _, opacity| {
            let params = TextParamsDelegate {
                render_mode: mode.into(),
                glyph_opacity: Deferred::value(opacity),
                ..Default::default()
            };
            render_drawable_graph(renderer, glyph_text("A\u{338}", params), size)
        };
        let opaque = render(&mut renderer, 1.0);
        let faded = render(&mut renderer, 0.5);
        assert!(
            opaque.iter().skip(3).step_by(4).any(|&a| a == 255),
            "{mode:?}"
        );
        for (index, (full, half)) in opaque.iter().zip(&faded).enumerate() {
            assert!(
                (f32::from(*full) * 0.5 - f32::from(*half)).abs() <= 1.5,
                "{mode:?} byte {index}: {full} at half opacity gave {half}"
            );
        }
    }
}

#[test]
fn glyph_opacity_keeps_a_cluster_split_by_wrapping_apart() {
    use lumen_engine::node::source::text::TextRenderMode;
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    for mode in [TextRenderMode::Raster, TextRenderMode::Msdf] {
        // Wrapping puts the base and its mark on separate lines, each moved by its own offset.
        let render = |renderer: &mut _, opacity| {
            let params = TextParamsDelegate {
                render_mode: mode.into(),
                max_width: Deferred::value(15.0),
                glyph_offset: expression("vec2(line * 40, 0)"),
                glyph_opacity: Deferred::value(opacity),
                ..Default::default()
            };
            render_drawable_graph(renderer, glyph_text("A\u{338}", params), size)
        };
        let opaque = render(&mut renderer, 1.0);
        let faded = render(&mut renderer, 0.5);
        for (index, (full, half)) in opaque.iter().zip(&faded).enumerate() {
            assert!(
                (f32::from(*full) * 0.5 - f32::from(*half)).abs() <= 1.5,
                "{mode:?} byte {index}: {full} at half opacity gave {half}"
            );
        }
    }
}

#[test]
fn text_glyph_rotation_turns_each_word_about_its_glyphs() {
    use lumen_engine::node::source::text::TextRenderMode;
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 160;
    for mode in [TextRenderMode::Raster, TextRenderMode::Msdf] {
        let params = || TextParamsDelegate {
            render_mode: mode.into(),
            ..Default::default()
        };
        let still = render_drawable_graph(&mut renderer, glyph_text("II   II", params()), size);
        let turned = render_drawable_graph(
            &mut renderer,
            glyph_text(
                "II   II",
                TextParamsDelegate {
                    glyph_rotation: expression("word * 90"),
                    ..params()
                },
            ),
            size,
        );
        let bars = inked_columns(&still, size);
        assert_eq!(bars.len(), 4, "{mode:?}");
        // The first word is untouched.
        for y in 0..size as usize {
            for x in bars[0].start..bars[1].end {
                assert_eq!(
                    alpha_of(&turned, size, x, y),
                    alpha_of(&still, size, x, y),
                    "{mode:?} ({x}, {y})"
                );
            }
        }
        // Each glyph of the second turns a quarter about its own middle, so the upright bars
        // lie down where they stood, beside each other.
        let word = bars[1].end + 4..size as usize;
        let [x0, y0, x1, y1] = ink_box(&still, size, word.clone());
        assert!(y1 - y0 > x1 - x0, "{mode:?} still {:?}", [x0, y0, x1, y1]);
        let [tx0, ty0, tx1, ty1] = ink_box(&turned, size, word);
        assert!(
            tx1 - tx0 > 4 * (ty1 - ty0),
            "{mode:?} turned {:?}",
            [tx0, ty0, tx1, ty1]
        );
        let middle = |low: usize, high: usize| (low + high) as f32 / 2.0;
        assert!((middle(tx0, tx1) - middle(x0, x1)).abs() <= 2.0, "{mode:?}");
    }
}

#[test]
fn text_glyph_params_read_the_line_index() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 192;
    let still = render_drawable_graph(
        &mut renderer,
        glyph_text("I\nI\nI", Default::default()),
        size,
    );
    let shifted = render_drawable_graph(
        &mut renderer,
        glyph_text(
            "I\nI\nI",
            TextParamsDelegate {
                glyph_offset: expression("vec2(line * 30, 0)"),
                glyph_opacity: expression("line < line_count - 1"),
                ..Default::default()
            },
        ),
        size,
    );
    // Lines are 48 pixels apart from y = 10. Each moves 30 pixels further right than the
    // last, and the last is hidden.
    for line in 0..3 {
        for y in 10 + 48 * line..10 + 48 * (line + 1) {
            for x in 0..size as usize - 60 {
                let expected = if line < 2 {
                    alpha_of(&still, size, x, y)
                } else {
                    0
                };
                let actual = alpha_of(&shifted, size, x + 30 * line, y);
                assert!(actual.abs_diff(expected) <= 1, "line {line} ({x}, {y})");
            }
        }
    }
}

#[test]
fn constant_glyph_params_match_the_equivalent_static_text() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    let content = "Lumen fi";
    let mut moved = glyph_text(content, Default::default());
    let NodeKind::Text(text) = &mut moved[0] else {
        unreachable!()
    };
    text.params.position = Deferred::value((32.0, 17.0));
    let moved = render_drawable_graph(&mut renderer, moved, size);
    let offset = render_drawable_graph(
        &mut renderer,
        glyph_text(
            content,
            TextParamsDelegate {
                glyph_offset: Deferred::value((12.0, 7.0)),
                ..Default::default()
            },
        ),
        size,
    );
    assert!(total_alpha(&moved) > 0);
    assert!(max_channel_difference(&moved, &offset) <= 1);
    // An expression that reads no index is the constant it evaluates to.
    let uniform = render_drawable_graph(
        &mut renderer,
        glyph_text(
            content,
            TextParamsDelegate {
                glyph_offset: expression("vec2(12, 7 + 0 * frame)"),
                glyph_scale: expression("vec2(1, 1)"),
                ..Default::default()
            },
        ),
        size,
    );
    assert_eq!(uniform, offset);
}

/// Inter, the only family [`render_text_frames`] resolves.
#[derive(Debug, Clone)]
struct BlurFont;

impl lumen_engine::media::FontResolver for BlurFont {
    fn id(&self) -> &str {
        "bounded-blur-inter"
    }
    fn data(&self) -> Result<Vec<Vec<u8>>, lumen_engine::error::MediaError> {
        Ok(vec![
            include_bytes!("../../text/assets/inter/InterVariable-Latin.ttf").to_vec(),
        ])
    }
}

impl MediaStore for BlurFont {
    fn get_image_resolver(&self, _: &str) -> Option<Box<dyn ImageResolver>> {
        None
    }
    fn get_video_resolver(&self, _: &str) -> Option<Box<dyn VideoFrameResolver>> {
        None
    }
    fn font_resolver_id(&self, family: &str) -> Option<String> {
        (family == "Inter").then(|| "bounded-blur-inter".to_string())
    }
    fn get_font_resolver(
        &self,
        family: &str,
    ) -> Option<Box<dyn lumen_engine::media::FontResolver>> {
        (family == "Inter")
            .then(|| Box::new(self.clone()) as Box<dyn lumen_engine::media::FontResolver>)
    }
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

/// Renders `frames` of a `size`² canvas holding a text node with the JSON `params`, which may
/// name Inter.
fn render_text_frames(
    renderer: &mut lumen_engine::gpu::GpuCompositionRenderer,
    params: serde_json::Value,
    size: u32,
    frames: &[u32],
) -> Vec<Vec<u8>> {
    let composition = lumen_engine::json::parse_value(&json!({
        "timeline": { "fps": 24, "duration_frames": 12 },
        "render_settings": { "width": size, "height": size },
        "nodes": [
            { "id": 1, "type": "text", "params": params },
            { "id": 2, "type": "media_output" }
        ],
        "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }]
    }))
    .unwrap();
    renderer
        .compile_with_media(
            &composition,
            &BlurFont,
            lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
        )
        .unwrap();
    frames
        .iter()
        .map(|&frame| render_rgba8(renderer, &composition, frame, &BlurFont))
        .collect()
}

fn render_text_params(
    renderer: &mut lumen_engine::gpu::GpuCompositionRenderer,
    params: serde_json::Value,
    size: u32,
) -> Vec<u8> {
    render_text_frames(renderer, params, size, &[0]).remove(0)
}

#[test]
fn text_spans_shape_in_their_own_size_and_weight() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    let text = |content: serde_json::Value, style: &serde_json::Value| {
        let mut params =
            json!({ "content": content, "font_family": "Inter", "position": [20, 20] });
        for (key, value) in style.as_object().unwrap() {
            params[key] = value.clone();
        }
        params
    };
    let small = json!({ "font_size": 24 });
    let large = json!({ "font_size": 48, "font_weight": 800 });
    let mut spans = [small.clone(), large.clone()];
    for span in &mut spans {
        span["text"] = json!("I");
    }
    let pixels = render_text_params(&mut renderer, text(json!(spans), &json!({})), size);
    let runs = inked_columns(&pixels, size);
    assert_eq!(runs.len(), 2, "{runs:?}");
    let inks = [0, 1].map(|index| ink_box(&pixels, size, runs[index].clone()));
    assert!(
        inks[0][3].abs_diff(inks[1][3]) <= 1,
        "baselines differ: {inks:?}"
    );
    // Each span's glyph is as wide and tall as the plain text in its style draws it.
    for (style, ink) in [small, large].iter().zip(inks) {
        let alone = render_text_params(&mut renderer, text(json!("I"), style), size);
        let alone = ink_box(&alone, size, inked_columns(&alone, size)[0].clone());
        let extent = |[x0, y0, x1, y1]: [usize; 4]| [x1 - x0, y1 - y0];
        let [width, height] = extent(ink);
        let [alone_width, alone_height] = extent(alone);
        assert!(
            width.abs_diff(alone_width) <= 1 && height.abs_diff(alone_height) <= 1,
            "{style}: {ink:?} drawn alone is {alone:?}"
        );
    }
}

#[test]
fn span_gradients_span_their_span_or_the_whole_text() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    let gradient = json!({
        "type": "linear_gradient",
        "start": [0, 0],
        "end": [1, 0],
        "stops": [[0, [255, 0, 0, 255]], [1, [0, 0, 255, 255]]]
    });
    for paint_space in ["span", "text"] {
        let pixels = render_text_params(
            &mut renderer,
            json!({
                "content": [{ "text": "IIII" }, { "text": "IIII", "color": gradient }],
                "font_family": "Inter",
                "font_size": 32,
                "position": [8, 40],
                "paint_space": paint_space
            }),
            size,
        );
        let runs = inked_columns(&pixels, size);
        assert_eq!(runs.len(), 8, "{runs:?}");
        // Sums of red, green and blue over the run's mostly opaque pixels.
        let channels = |run: &std::ops::Range<usize>| {
            let mut sums = [0u64; 3];
            for y in 0..size as usize {
                for x in run.clone() {
                    let pixel = &pixels[(y * size as usize + x) * 4..][..4];
                    if pixel[3] > 200 {
                        for (sum, &channel) in sums.iter_mut().zip(pixel) {
                            *sum += u64::from(channel);
                        }
                    }
                }
            }
            sums
        };
        let red_share = |run| {
            let [red, _, blue] = channels(run);
            red as f64 / (red + blue) as f64
        };
        // The first span takes the node's white.
        for run in &runs[..4] {
            let [red, green, blue] = channels(run);
            assert!(
                green > 0 && green == red && green == blue,
                "{paint_space}: {run:?}"
            );
        }
        let (first, last) = (red_share(&runs[4]), red_share(&runs[7]));
        match paint_space {
            "span" => assert!(first > 0.75, "{paint_space}: {first}"),
            _ => assert!((0.3..0.7).contains(&first), "{paint_space}: {first}"),
        }
        assert!(last < 0.25, "{paint_space}: {last}");
    }
}

#[test]
fn glyph_params_read_the_span_index() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    let text = |glyph_offset: serde_json::Value| {
        json!({
            "content": [{ "text": "II" }, { "text": "II" }],
            "font_family": "Inter",
            "font_size": 32,
            "position": [8, 20],
            "glyph_offset": glyph_offset
        })
    };
    let still = render_text_params(&mut renderer, text(json!([0, 0])), size);
    let moved = render_text_params(
        &mut renderer,
        text(json!("=vec2(0, 40 * span / (span_count - 1))")),
        size,
    );
    let runs = inked_columns(&still, size);
    assert_eq!(runs.len(), 4, "{runs:?}");
    assert_eq!(inked_columns(&moved, size), runs);
    for (index, run) in runs.into_iter().enumerate() {
        let [x0, y0, x1, y1] = ink_box(&still, size, run.clone());
        let shift = if index < 2 { 0 } else { 40 };
        assert_eq!(
            ink_box(&moved, size, run),
            [x0, y0 + shift, x1, y1 + shift],
            "glyph {index}"
        );
    }
}

#[test]
fn animated_span_fields_reshape_each_frame() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    let text = |font_size: serde_json::Value| {
        json!({
            "content": [{ "text": "I" }, { "text": "I", "font_size": font_size }],
            "font_family": "Inter",
            "font_size": 24,
            "position": [20, 20]
        })
    };
    let frames = render_text_frames(
        &mut renderer,
        text(json!("=24 + frame * 24")),
        size,
        &[0, 1, 0],
    );
    assert_ne!(frames[0], frames[1]);
    assert_eq!(frames[0], frames[2]);
    for (frame, font_size) in [(0, 24), (1, 48)] {
        let expected = render_text_params(&mut renderer, text(json!(font_size)), size);
        assert!(frames[frame] == expected, "frame {frame}");
    }
}

#[test]
fn plain_string_content_matches_its_span_and_expression_forms() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    let text = |content: serde_json::Value| {
        json!({
            "content": content,
            "font_size": 32,
            "position": [8, 20],
            "color": {
                "type": "linear_gradient",
                "stops": [[0, [255, 0, 0, 255]], [1, [0, 0, 255, 255]]]
            }
        })
    };
    let plain = render_text_params(&mut renderer, text(json!("Lumen")), size);
    assert!(total_alpha(&plain) > 0);
    for content in [
        json!([{ "text": "Lumen" }]),
        json!("=\"Lumen\""),
        json!([{ "text": "=\"Lumen\"" }]),
    ] {
        let pixels = render_text_params(&mut renderer, text(content.clone()), size);
        assert!(pixels == plain, "{content}");
    }
}

/// Text params in Inter at 32 px, with `extra` params added.
fn decorated_text(content: serde_json::Value, extra: serde_json::Value) -> serde_json::Value {
    let mut params = json!({
        "content": content,
        "font_family": "Inter",
        "font_size": 32,
        "position": [8, 20]
    });
    for (key, value) in extra.as_object().unwrap() {
        params[key] = value.clone();
    }
    params
}

/// `[min_x, min_y, max_x, max_y]` of the pixels where `a` and `b` differ.
fn diff_box(a: &[u8], b: &[u8], size: u32) -> Option<[usize; 4]> {
    let size = size as usize;
    let mut bounds: Option<[usize; 4]> = None;
    for y in 0..size {
        for x in 0..size {
            let index = (y * size + x) * 4;
            if a[index..index + 4] != b[index..index + 4] {
                let [x0, y0, x1, y1] = bounds.get_or_insert([x, y, x, y]);
                (*x0, *y0, *x1, *y1) = ((*x0).min(x), (*y0).min(y), (*x1).max(x), (*y1).max(y));
            }
        }
    }
    bounds
}

/// Runs of rows that hold any ink, top to bottom.
fn inked_rows(pixels: &[u8], size: u32) -> Vec<std::ops::Range<usize>> {
    let inked = |y: usize| (0..size as usize).any(|x| alpha_of(pixels, size, x, y) > 0);
    let mut runs: Vec<std::ops::Range<usize>> = Vec::new();
    for y in (0..size as usize).filter(|&y| inked(y)) {
        match runs.last_mut() {
            Some(run) if run.end == y => run.end = y + 1,
            _ => runs.push(y..y + 1),
        }
    }
    runs
}

fn rgba_at(pixels: &[u8], size: u32, x: usize, y: usize) -> [u8; 4] {
    pixels[(y * size as usize + x) * 4..][..4]
        .try_into()
        .unwrap()
}

#[test]
fn span_underline_draws_under_that_span_alone() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    let plain = render_text_params(
        &mut renderer,
        decorated_text(json!([{ "text": "II" }, { "text": "II" }]), json!({})),
        size,
    );
    let underlined = render_text_params(
        &mut renderer,
        decorated_text(
            json!([{ "text": "II" }, { "text": "II", "underline": {} }]),
            json!({}),
        ),
        size,
    );
    let runs = inked_columns(&plain, size);
    assert_eq!(runs.len(), 4, "{runs:?}");
    let baseline = ink_box(&plain, size, runs[0].clone())[3];
    let [x0, y0, x1, y1] = diff_box(&plain, &underlined, size).expect("an underline");
    assert!(
        x0 + 1 >= runs[1].end && x1 <= runs[3].end + 2,
        "{x0}..{x1} is not under {runs:?}"
    );
    assert!(y0 > baseline && y1 < baseline + 12, "{y0}..{y1}");
    let middle = (y0 + y1) / 2;
    assert_eq!(rgba_at(&underlined, size, (x0 + x1) / 2, middle), [255; 4]);
}

#[test]
fn decoration_dots_stretch_with_a_nonuniform_scale() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 256;
    // Clear glyphs leave only the underline.
    let pixels = render_text_params(
        &mut renderer,
        decorated_text(
            json!("IIIIII"),
            json!({
                "color": [0, 0, 0, 0],
                "scale": [4, 1],
                "underline": {
                    "style": "dotted",
                    "thickness": 4,
                    "paint": [255, 0, 0, 255]
                }
            }),
        ),
        size,
    );
    let dots = inked_columns(&pixels, size);
    assert!(dots.len() >= 3, "{dots:?}");
    // Circles of 4 px, stretched to 16 px along x.
    for dot in &dots[1..dots.len() - 1] {
        assert!((14..=19).contains(&dot.len()), "{dots:?}");
    }
    let rows = inked_rows(&pixels, size);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!((4..=6).contains(&rows[0].len()), "{rows:?}");
}

#[test]
fn decoration_highlights_break_into_a_padded_box_per_line() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    // Clear glyphs leave only the highlight.
    let pixels = render_text_params(
        &mut renderer,
        decorated_text(
            json!("II II"),
            json!({
                "color": [0, 0, 0, 0],
                "position": [8, 8],
                "max_width": 40,
                "line_height": 48,
                "highlight": { "paint": [255, 0, 0, 255], "padding": [4, 2], "radius": 2 }
            }),
        ),
        size,
    );
    let rows = inked_rows(&pixels, size);
    assert_eq!(rows.len(), 2, "{rows:?}");
    let boxes = [&rows[0], &rows[1]].map(|rows| {
        let middle = (rows.start + rows.end) / 2;
        let inked: Vec<usize> = (0..size as usize)
            .filter(|&x| alpha_of(&pixels, size, x, middle) > 128)
            .collect();
        [inked[0], *inked.last().unwrap(), rows.len()]
    });
    // Each line's box starts at the text's left edge less its padding.
    assert!(boxes[0][0].abs_diff(4) <= 1, "{boxes:?}");
    assert_eq!(boxes[0], boxes[1]);
    assert_eq!(
        rgba_at(&pixels, size, boxes[0][0] + 4, rows[0].start + 4),
        [255, 0, 0, 255]
    );
}

#[test]
fn decoration_strikethroughs_draw_over_glyphs_and_highlights_under_them() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    let text = |extra: serde_json::Value| decorated_text(json!("IIII"), extra);
    let plain = render_text_params(&mut renderer, text(json!({ "font_size": 48 })), size);
    let pixels = render_text_params(
        &mut renderer,
        text(json!({
            "font_size": 48,
            "highlight": { "paint": [0, 0, 255, 255] },
            "strikethrough": { "paint": [255, 0, 0, 255], "thickness": 4 }
        })),
        size,
    );
    let runs = inked_columns(&plain, size);
    assert_eq!(runs.len(), 4, "{runs:?}");
    let pixels = &pixels;
    let column = |x: usize| (0..size as usize).map(move |y| rgba_at(pixels, size, x, y));
    let stem = (runs[0].start + runs[0].end) / 2;
    let gap = runs[0].end + 1;
    assert!(gap < runs[1].start, "{runs:?}");
    let red = [255, 0, 0, 255];
    assert!(
        column(stem).any(|pixel| pixel == red),
        "no strikethrough over the glyph"
    );
    assert!(
        column(stem).any(|pixel| pixel == [255; 4]),
        "no glyph over the highlight"
    );
    assert!(column(gap).any(|pixel| pixel == red));
    assert!(column(gap).any(|pixel| pixel == [0, 0, 255, 255]));
}

#[test]
fn decoration_extent_runs_on_across_a_line_break() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    let text = |extent: serde_json::Value| {
        decorated_text(
            json!("IIII\nIIII"),
            json!({
                "color": [0, 0, 0, 0],
                "line_height": 48,
                "underline": { "paint": [255, 0, 0, 255], "thickness": 3, "extent": extent }
            }),
        )
    };
    let full = render_text_params(&mut renderer, text(json!([0, 1])), size);
    let partial = render_text_params(&mut renderer, text(json!([0, 0.75])), size);
    // 0.75 of the two lines' width: all of the first line and half of the second.
    let rows = inked_rows(&full, size);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(inked_rows(&partial, size), rows);
    let widths = [&full, &partial].map(|pixels| {
        [&rows[0], &rows[1]].map(|rows| {
            let y = (rows.start + rows.end) / 2;
            (0..size as usize)
                .filter(|&x| alpha_of(pixels, size, x, y) > 128)
                .count() as f64
        })
    });
    let [[first, second], [first_partial, second_partial]] =
        widths.map(|widths| [widths[0], widths[1]]);
    assert_eq!(first, second);
    assert_eq!(first_partial, first);
    assert!(
        (second_partial / second - 0.5).abs() < 0.05,
        "{second_partial} of {second}"
    );
    let animated = render_text_frames(
        &mut renderer,
        text(json!("=vec2(0, frame / 4)")),
        size,
        &[3],
    );
    assert!(animated[0] == partial);
}

#[test]
fn span_null_clears_the_node_underline() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    let cleared = render_text_params(
        &mut renderer,
        decorated_text(
            json!([{ "text": "II" }, { "text": "II", "underline": null }, { "text": "II" }]),
            json!({ "underline": {} }),
        ),
        size,
    );
    let own = render_text_params(
        &mut renderer,
        decorated_text(
            json!([
                { "text": "II", "underline": {} },
                { "text": "II" },
                { "text": "II", "underline": {} }
            ]),
            json!({}),
        ),
        size,
    );
    let all = render_text_params(
        &mut renderer,
        decorated_text(json!("IIIIII"), json!({ "underline": {} })),
        size,
    );
    assert!(cleared == own);
    assert!(cleared != all);
}

#[test]
fn bounded_text_holds_wavy_lines_and_highlight_padding() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    for decoration in [
        json!({ "underline": { "style": "wavy", "thickness": 4, "offset": 20 } }),
        json!({ "highlight": { "paint": [255, 0, 0, 255], "padding": [12, 16] } }),
    ] {
        let bounded = render_text_params(
            &mut renderer,
            decorated_text(json!("II"), decoration.clone()),
            size,
        );
        let text = PortRef::new(NodeId::new(1), "output".to_string());
        let domain = renderer.compiled().unwrap().node_outputs[&text]
            .clone()
            .into_raster(text.id, "output")
            .unwrap()
            .domain;
        assert!(!domain.is_full_frame(), "{decoration}");
        // Content with an expression is not bounded.
        let full = render_text_params(
            &mut renderer,
            decorated_text(json!("=\"II\""), decoration.clone()),
            size,
        );
        assert!(bounded == full, "{decoration}");
    }
}

#[test]
fn text_overflow_ellipsis_ends_the_last_line_shown() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    let text = |content: &str, extra: serde_json::Value| {
        let mut params = decorated_text(
            json!(content),
            json!({ "position": [8, 8], "line_height": 40, "max_lines": 2 }),
        );
        for (key, value) in extra.as_object().unwrap() {
            params[key] = value.clone();
        }
        params
    };
    let content = "II II\nII II\nII II";
    // `max_lines` alone drops the lines past it.
    let clipped = render_text_params(&mut renderer, text(content, json!({})), size);
    let ellipsis = json!({ "overflow": "ellipsis" });
    let ellipsized = render_text_params(&mut renderer, text(content, ellipsis.clone()), size);
    let rows = inked_rows(&clipped, size);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(inked_rows(&ellipsized, size).len(), 2);
    // The ellipsis follows the second line.
    let second_line_end = (0..size as usize)
        .filter(|&x| rows[1].clone().any(|y| alpha_of(&clipped, size, x, y) > 0))
        .max()
        .unwrap();
    let [x0, y0, ..] = diff_box(&clipped, &ellipsized, size).expect("an ellipsis");
    assert!(x0 > second_line_end && y0 >= rows[1].start, "{x0}, {y0}");
    // It is the last glyph, so hiding it leaves the clipped text.
    let hidden = render_text_params(
        &mut renderer,
        text(
            content,
            json!({ "overflow": "ellipsis", "glyph_opacity": "=min(1, glyph_count - 1 - glyph)" }),
        ),
        size,
    );
    assert!(hidden == clipped);
    // Its bounds hold the ellipsis.
    let unbounded = render_text_params(
        &mut renderer,
        text(&format!("={content:?}"), ellipsis),
        size,
    );
    assert!(unbounded == ellipsized);
}

#[test]
fn text_fit_shrinks_into_its_box_and_measures_as_drawn() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    let text = |min_font_size: f64| {
        decorated_text(
            json!("IIIIIIIIIIIIIIII"),
            json!({
                "position": "=vec2(100 - text_width(node(1)), 20)",
                "max_width": 64,
                "max_lines": 1,
                "fit": "shrink",
                "min_font_size": min_font_size
            }),
        )
    };
    let fitted = render_text_params(&mut renderer, text(4.0), size);
    let columns = inked_columns(&fitted, size);
    assert_eq!(columns.len(), 16, "{columns:?}");
    let [start, end] = [columns[0].start, columns[15].end];
    assert!(end - start <= 64, "{start}..{end}");
    // `text_width` measures the shrunk text, which ends where it says.
    assert!(end.abs_diff(100) <= 2, "{end}");
    let fitted_rows = inked_rows(&fitted, size);
    assert_eq!(fitted_rows.len(), 1, "{fitted_rows:?}");

    // At its smallest size it still overflows, so the lines past the first are clipped.
    let clipped = render_text_params(&mut renderer, text(30.0), size);
    let rows = inked_rows(&clipped, size);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(2 * rows[0].len() > 3 * fitted_rows[0].len(), "{rows:?}");
    let columns = inked_columns(&clipped, size);
    assert!(columns.len() < 16, "{columns:?}");
    assert!(
        columns.last().unwrap().end.abs_diff(100) <= 2,
        "{columns:?}"
    );
}

#[test]
fn typography_params_move_ink_and_keep_bounded_text_whole() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    let plain = render_text_params(
        &mut renderer,
        decorated_text(json!([{ "text": "II" }, { "text": "II" }]), json!({})),
        size,
    );
    // Raised past the top of the line box, the span is still drawn whole.
    let raised = render_text_params(
        &mut renderer,
        decorated_text(
            json!([{ "text": "II" }, { "text": "II", "baseline_shift": 24 }]),
            json!({}),
        ),
        size,
    );
    let (plain_runs, raised_runs) = (inked_columns(&plain, size), inked_columns(&raised, size));
    assert_eq!(raised_runs, plain_runs);
    for (run, lift) in [(0, 0), (2, 24)] {
        let [_, top, ..] = ink_box(&plain, size, plain_runs[run].clone());
        let [_, raised_top, ..] = ink_box(&raised, size, raised_runs[run].clone());
        assert_eq!(raised_top + lift, top, "run {run}");
    }

    let upper = render_text_params(
        &mut renderer,
        decorated_text(json!("ii"), json!({ "text_transform": "uppercase" })),
        size,
    );
    let capitals = render_text_params(&mut renderer, decorated_text(json!("II"), json!({})), size);
    assert!(upper == capitals);

    let mut paragraphs = |spacing: u32| {
        let pixels = render_text_params(
            &mut renderer,
            decorated_text(json!("I\nI"), json!({ "paragraph_spacing": spacing })),
            size,
        );
        inked_rows(&pixels, size)
    };
    let (tight, spaced) = (paragraphs(0), paragraphs(10));
    assert_eq!(spaced[0], tight[0]);
    assert_eq!(spaced[1].start, tight[1].start + 10);
}

#[test]
fn stroke_rings_glyphs_outside_under_their_fill_or_centred_over_it() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    let near = |pixel: [u8; 4], expected: [u8; 4]| {
        pixel
            .iter()
            .zip(expected)
            .all(|(&channel, expected)| channel.abs_diff(expected) <= 8)
    };
    for render_mode in ["raster", "msdf"] {
        let base = json!({ "font_size": 64, "position": [40, 20], "render_mode": render_mode });
        let stroked = |stroke: serde_json::Value| {
            let mut extra = base.clone();
            extra["stroke"] = stroke;
            decorated_text(json!("I"), extra)
        };
        let plain = render_text_params(
            &mut renderer,
            decorated_text(json!("I"), base.clone()),
            size,
        );
        let [x0, y0, x1, y1] = ink_box(&plain, size, 0..size as usize);
        let (stem, middle) = ((x0 + x1) / 2, (y0 + y1) / 2);
        for (align, reach, edge) in [
            ("outside", 4, [255, 0, 0, 255]),
            ("center", 2, [255, 0, 0, 255]),
        ] {
            let pixels = render_text_params(
                &mut renderer,
                stroked(json!({ "paint": [255, 0, 0, 255], "width": 4, "align": align })),
                size,
            );
            let [sx0, sy0, sx1, sy1] = ink_box(&pixels, size, 0..size as usize);
            for grown in [x0 - sx0, y0 - sy0, sx1 - x1, sy1 - y1] {
                assert!(
                    grown.abs_diff(reach) <= 1,
                    "{render_mode} {align}: {:?} around {:?}",
                    [sx0, sy0, sx1, sy1],
                    [x0, y0, x1, y1]
                );
            }
            // An outside stroke shows past the edge and the fill covers its inner half; a
            // centred one covers the glyph's edge.
            let at = if align == "outside" { x0 - 2 } else { x0 };
            let pixel = rgba_at(&pixels, size, at, middle);
            assert!(
                near(pixel, edge),
                "{render_mode} {align}: {pixel:?} at the edge"
            );
            let pixel = rgba_at(&pixels, size, stem, middle);
            assert!(
                near(pixel, [255; 4]),
                "{render_mode} {align}: {pixel:?} on the stem"
            );
        }
    }
}

#[test]
fn span_null_clears_the_node_stroke() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    let cleared = render_text_params(
        &mut renderer,
        decorated_text(
            json!([{ "text": "II" }, { "text": "II", "stroke": null }, { "text": "II" }]),
            json!({ "stroke": {} }),
        ),
        size,
    );
    let own = render_text_params(
        &mut renderer,
        decorated_text(
            json!([
                { "text": "II", "stroke": {} },
                { "text": "II" },
                { "text": "II", "stroke": {} }
            ]),
            json!({}),
        ),
        size,
    );
    let all = render_text_params(
        &mut renderer,
        decorated_text(json!("IIIIII"), json!({ "stroke": {} })),
        size,
    );
    assert!(cleared == own);
    assert!(cleared != all);
}

#[test]
fn bounded_text_holds_its_stroke() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    let extra = json!({
        "position": [32, 32],
        "stroke": { "paint": [255, 0, 0, 255], "width": 12 }
    });
    let bounded = render_text_params(
        &mut renderer,
        decorated_text(json!("II"), extra.clone()),
        size,
    );
    let text = PortRef::new(NodeId::new(1), "output".to_string());
    let domain = renderer.compiled().unwrap().node_outputs[&text]
        .clone()
        .into_raster(text.id, "output")
        .unwrap()
        .domain;
    assert!(!domain.is_full_frame());
    // Content with an expression is not bounded.
    let full = render_text_params(&mut renderer, decorated_text(json!("=\"II\""), extra), size);
    assert!(bounded == full);
}

#[test]
fn minified_distance_field_stroke_stays_off_padding_and_counters() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 64;
    // At 48 texels per em the stroke reaches as far as its field holds, and once scaled down a
    // pixel spans more texels than the widest antialiasing.
    let params = json!({
        "font_size": 120,
        "position": [20, 20],
        "scale": [0.1, 0.1],
        "render_mode": "msdf"
    });
    let plain = render_text_params(
        &mut renderer,
        decorated_text(json!("O"), params.clone()),
        size,
    );
    let mut stroked = params;
    stroked["stroke"] = json!({ "paint": [255, 0, 0, 255], "width": 18.75 });
    let pixels = render_text_params(&mut renderer, decorated_text(json!("O"), stroked), size);
    assert!(
        pixels
            .chunks_exact(4)
            .any(|pixel| pixel[0] > 100 && pixel[1] == 0),
        "no stroke drawn"
    );
    // The glyph quad's padding past the ink's corner, and the middle of the counter, which
    // read saturated texels.
    let [x0, y0, x1, y1] = ink_box(&plain, size, 0..size as usize);
    for (x, y) in [(x0 - 2, y0 - 2), ((x0 + x1) / 2, (y0 + y1) / 2)] {
        assert_eq!(rgba_at(&pixels, size, x, y), [0; 4], "at {x}, {y}");
    }
}

#[test]
fn hairline_stroke_draws_almost_nothing_at_the_contour() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    for render_mode in ["raster", "msdf"] {
        let base = json!({ "font_size": 64, "position": [40, 20], "render_mode": render_mode });
        let plain = render_text_params(
            &mut renderer,
            decorated_text(json!("I"), base.clone()),
            size,
        );
        let mut extra = base;
        extra["stroke"] = json!({ "paint": [255, 0, 0, 255], "width": 0.01, "align": "center" });
        let stroked = render_text_params(&mut renderer, decorated_text(json!("I"), extra), size);
        let most = plain
            .iter()
            .zip(&stroked)
            .map(|(plain, stroked)| plain.abs_diff(*stroked))
            .max()
            .unwrap();
        assert!(
            most <= 4,
            "{render_mode}: a hairline changed a channel by {most}"
        );
    }
}

#[test]
fn wide_distance_field_strokes_keep_widening() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    let mut ink = |width: u32| {
        let params = decorated_text(
            json!("I"),
            json!({
                "font_size": 24,
                "position": [40, 40],
                "render_mode": "msdf",
                "stroke": { "paint": [255, 0, 0, 255], "width": width }
            }),
        );
        let pixels = render_text_params(&mut renderer, params, size);
        ink_box(&pixels, size, 0..size as usize)
    };
    let (narrow, wide) = (ink(4), ink(8));
    for (narrow, wide, side) in [
        (narrow[0], wide[0], 0),
        (narrow[1], wide[1], 1),
        (wide[2], narrow[2], 2),
        (wide[3], narrow[3], 3),
    ] {
        assert!(
            narrow.abs_diff(wide) >= 3,
            "side {side}: {narrow:?} and {wide:?} from widths 4 and 8"
        );
    }
}

/// The number of pixels where `a` and `b` differ, and the largest difference in a channel.
fn pixel_changes(a: &[u8], b: &[u8]) -> (usize, u8) {
    a.chunks_exact(4)
        .zip(b.chunks_exact(4))
        .map(|(a, b)| a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)).max().unwrap())
        .filter(|&change| change > 0)
        .fold((0, 0), |(count, most), change| {
            (count + 1, most.max(change))
        })
}

#[test]
fn distance_field_stroke_widens_smoothly_into_wider_ranges() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    // At 240 px a stroke's field has 96 texels per em whatever its width, and these widths fill
    // each range tier, so a wider stroke takes a field with twice the range. Minified a hundred
    // times, moving the text 1/512 px changes pixels by up to 7 levels, so crossing a tier may
    // change them by a few.
    for (position, scale, size, most_allowed) in
        [([100.3, 70.3], 1.0, 448, 2), ([32.3, 32.3], 0.01, 64, 4)]
    {
        let mut render = |width: f32| {
            let params = decorated_text(
                json!("O"),
                json!({
                    "font_size": 240,
                    "position": position,
                    "scale": [scale, scale],
                    "render_mode": "msdf",
                    "stroke": { "paint": [255, 0, 0, 255], "width": width }
                }),
            );
            render_text_params(&mut renderer, params, size)
        };
        for limit in [3.75, 18.75, 48.75, 108.75] {
            let (narrower, narrow, wide) =
                (render(limit - 0.001), render(limit), render(limit + 0.001));
            // Widening by 0.001 px rounds the contour's pixels by a level within a field;
            // crossing into the wider range should change no more.
            let within = pixel_changes(&narrower, &narrow).0;
            let (changed, most) = pixel_changes(&narrow, &wide);
            assert!(
                changed <= 2 * within.max(1) && most <= most_allowed,
                "scale {scale}, width {limit}: {changed} pixels changed by up to {most}, \
                 against {within} within a field"
            );
        }
    }
}

#[test]
fn distance_field_stroke_follows_its_fill_across_font_sizes() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 192;
    // Growing small text by 0.001 px moves its stroke about as much as its fill. Software
    // rasterizers sample a few levels apart, and a stroke field coarser than the fill's would
    // change by tens.
    let mut render = |font_size: f32, stroke: bool| {
        let mut params = json!({
            "font_size": font_size,
            "position": [30.3, 30.3],
            "render_mode": "msdf",
        });
        if stroke {
            params["stroke"] = json!({ "paint": [255, 0, 0, 255], "width": 1, "align": "center" });
        }
        render_text_params(&mut renderer, decorated_text(json!("O"), params), size)
    };
    for font_size in [12.0, 24.0, 48.0] {
        let fill = pixel_changes(&render(font_size - 0.001, false), &render(font_size, false)).1;
        let (changed, most) =
            pixel_changes(&render(font_size - 0.001, true), &render(font_size, true));
        assert!(
            most <= fill + 8,
            "font size {font_size}: {changed} pixels changed by up to {most}, against {fill} for the fill"
        );
    }
}

#[test]
fn magnified_hairline_strokes_fade_in_continuously() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    // Centred strokes on either side of a 1/512 px half width, magnified 50 times, show and
    // fade with their width.
    let mut render = |width: f32| {
        let params = decorated_text(
            json!("I"),
            json!({
                "font_size": 2,
                "position": [48, 24],
                "scale": [50, 50],
                "render_mode": "msdf",
                "stroke": { "paint": [255, 0, 0, 255], "width": width, "align": "center" }
            }),
        );
        render_text_params(&mut renderer, params, size)
    };
    let plain = render(0.0);
    let (thinner, thicker) = (render(0.0039062), render(0.0039063));
    let shown = pixel_changes(&plain, &thinner).1;
    let (changed, most) = pixel_changes(&thinner, &thicker);
    assert!(
        shown >= 8,
        "the thinner stroke changed a channel by {shown}"
    );
    assert!(most <= 1, "{changed} pixels changed by up to {most}");
}

#[test]
fn shrink_to_fit_narrows_the_stroke_with_the_text() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    let fit = json!({ "max_width": 16, "max_lines": 1, "fit": "shrink", "min_font_size": 4 });
    let mut ink = |fit: Option<&serde_json::Value>, stroke: bool| {
        let mut params = json!({ "font_size": 64, "position": [20, 20] });
        for (key, value) in fit.and_then(|fit| fit.as_object()).into_iter().flatten() {
            params[key] = value.clone();
        }
        if stroke {
            params["stroke"] = json!({ "paint": [255, 0, 0, 255], "width": 8 });
        }
        let pixels = render_text_params(&mut renderer, decorated_text(json!("II"), params), size);
        ink_box(&pixels, size, 0..size as usize)
    };
    let (plain, stroked) = (ink(None, false), ink(None, true));
    let (fitted, fitted_stroked) = (ink(Some(&fit), false), ink(Some(&fit), true));
    let scale = (fitted[3] - fitted[1]) as f32 / (plain[3] - plain[1]) as f32;
    assert!(scale < 0.6, "{scale}");
    assert!((plain[1] - stroked[1]).abs_diff(8) <= 1);
    let reach = (fitted[1] - fitted_stroked[1]) as f32;
    assert!(
        (reach - 8.0 * scale).abs() <= 1.5,
        "{reach} at scale {scale}"
    );
}

#[test]
fn faded_glyph_fades_its_stroke_and_fill_as_one() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    for render_mode in ["raster", "msdf"] {
        for align in ["outside", "center"] {
            let params = json!({
                "font_size": 64,
                "position": [40, 20],
                "render_mode": render_mode,
                "glyph_opacity": 0.5,
                "stroke": { "paint": [255, 0, 0, 255], "width": 4, "align": align }
            });
            let pixels =
                render_text_params(&mut renderer, decorated_text(json!("I"), params), size);
            // Where the fill and stroke overlap, the pair shows at half opacity, not as two
            // layers at half opacity each.
            let most = pixels.chunks_exact(4).map(|pixel| pixel[3]).max().unwrap();
            assert!(
                most.abs_diff(128) <= 2,
                "{render_mode} {align}: alpha {most}"
            );
        }
    }
}

#[test]
fn faded_strokes_and_clusters_keep_their_supersampled_paint() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    // Opaque on the left half of the line and clear on the right, switching inside a pixel.
    let gradient = json!({
        "type": "linear_gradient",
        "start": [0, 0],
        "end": [1, 0],
        "stops": [
            [0, [255, 255, 255, 255]],
            [0.5, [255, 255, 255, 255]],
            [0.5, [255, 255, 255, 0]],
            [1, [255, 255, 255, 0]]
        ]
    });
    let stroke = json!({ "paint": [255, 0, 0, 255], "width": 4 });
    for (content, stroke) in [
        ("I", stroke.clone()),
        ("A\u{338}", json!(null)),
        ("A\u{338}", stroke),
    ] {
        let mut render = |opacity: f64| {
            let mut params = json!({
                "font_size": 64,
                "position": [40.8, 20.3],
                "color": gradient,
                "glyph_opacity": opacity
            });
            if !stroke.is_null() {
                params["stroke"] = stroke.clone();
            }
            render_text_params(&mut renderer, decorated_text(json!(content), params), size)
        };
        let (opaque, faded) = (render(1.0), render(0.5));
        let most = opaque
            .iter()
            .zip(&faded)
            .map(|(&opaque, &faded)| (opaque as f32 * 0.5 - faded as f32).abs())
            .fold(0.0, f32::max);
        assert!(most <= 2.0, "{content:?} {stroke}: off by {most}");
    }
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
