//! The shared glyph atlas, and text whose fonts arrive after it was measured. A font a row loads
//! lives in the text session of its own `TestMedia`, so no other row or test reads it or finds it
//! left behind, and rows that load a font need not hold the device to keep fonts apart.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use lumen_engine::{
    composition::Composition,
    error::MediaError,
    expr::{Expression, ExpressionContext},
    gpu::{BoundFrame, CompileContext, CompiledOutput, FrameBindContext, GpuCompositionRenderer},
    media::{FontResolver, MediaStore, TextSession},
    node::{NodeId, PortRef},
};
use lumen_gpu::{Upload, wgpu};
use serde_json::{Value, json};

use crate::support::{
    Gpu, Scene, TestMedia, check_each, gpu, read_rgba8, render, roboto_scaled, scene,
    within_rounding,
};

fn drawn(pixels: &[u8]) -> bool {
    pixels.chunks(4).any(|pixel| pixel[3] > 0)
}

fn atlas_texture(renderer: &GpuCompositionRenderer) -> Arc<wgpu::Texture> {
    let plan = &renderer.compiled().expect("a plan is compiled").plan;
    let mut atlases = plan
        .textures()
        .iter()
        .filter(|texture| texture.label.as_deref() == Some("text:shared-atlas"));
    let atlas = atlases.next().expect("the plan has a shared atlas").id;
    assert!(atlases.next().is_none(), "the plan has one shared atlas");
    renderer
        .gpu_renderer()
        .texture_arc(atlas)
        .expect("the atlas exists")
}

fn text_node(id: u32, params: Value) -> Value {
    json!({ "id": id, "type": "text", "params": params })
}

/// A 1024 × 256, 12 frame scene of one text node.
fn one_text(params: Value) -> Scene {
    scene((1024, 256), json!([text_node(1, params)])).frames(12)
}

/// A text node and a second one, merged, so that both draw from the one atlas.
fn two_texts(second: Value) -> Composition {
    scene(
        (1024, 256),
        json!([
            text_node(1, json!({ "content": "Atlas W", "font_size": 72 })),
            text_node(2, second),
            { "id": 3, "type": "merge", "in": { "base": 1, "overlay": 2 } },
        ]),
    )
    .into()
}

#[test]
fn a_standalone_plan_draws_text_only_after_its_atlas_is_injected() {
    let gpu = gpu();
    let mut bare = gpu.bare();
    for mode in ["raster", "msdf", "auto"] {
        let validation = bare.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let params = json!({ "content": "Standalone text", "font_size": 72, "render_mode": mode });
        let composition: Composition = one_text(params).into();
        let compiled = CompileContext::new(&composition).compile().unwrap();
        bare.prepare_plan(&compiled.plan).unwrap();
        let bound = FrameBindContext::new(&composition, 0)
            .bind(&compiled)
            .unwrap();
        let error = bare
            .execute(&compiled.plan, &bound.frame_update())
            .unwrap_err();
        assert!(
            error.to_string().contains("replace placeholder textures"),
            "{mode}: {error}"
        );

        compiled.prepare_text_atlas(&mut bare).unwrap();
        bare.execute(&compiled.plan, &bound.frame_update()).unwrap();
        bound.mark_uploaded().unwrap();
        let texture = bare.texture(compiled.output.texture).unwrap();
        let pixels = read_rgba8(&bare, texture, compiled.output.domain.storage_size);
        assert!(drawn(&pixels), "{mode}: nothing is drawn");
        assert!(pollster::block_on(validation.pop()).is_none(), "{mode}");
    }
}

#[test]
fn the_shared_atlas_is_kept_across_recompiles_and_dropped_on_release() {
    let gpu = gpu();
    let media = TestMedia::default();
    let mut renderer = gpu.renderer();
    let small = two_texts(json!({ "content": "Atlas M", "font_size": 72, "position": [0, 100] }));
    let initial = render(&mut renderer, &small, 0.0, &media);
    assert!(drawn(&initial.rgba));
    let atlas = atlas_texture(&renderer);

    renderer.release_resources();
    let rebound = renderer.bind_frame(&small, 0, &media).unwrap();
    assert_eq!(
        rebound.texture_upload_count(),
        0,
        "recompiling keeps the glyphs resident"
    );
    assert!(Arc::ptr_eq(&atlas_texture(&renderer), &atlas));
    assert!(
        render(&mut renderer, &small, 0.0, &media).rgba == initial.rgba,
        "recompiling changes the frame"
    );

    // A glyph far larger than the others is drawn from the same atlas texture.
    let huge = two_texts(json!({ "content": "W", "font_size": 3800, "position": [0, 100] }));
    renderer.release_resources();
    let bound = renderer.bind_frame(&huge, 0, &media).unwrap();
    renderer.upload_bound_frame(&bound).unwrap();
    assert!(Arc::ptr_eq(&atlas_texture(&renderer), &atlas));
    let instances = renderer
        .compiled()
        .unwrap()
        .plan
        .buffers()
        .iter()
        .find(|buffer| buffer.label.as_deref() == Some("text:2:instances"))
        .unwrap()
        .id;
    let has_huge_glyph = bound.frame_update().uploads().iter().any(|upload| {
        let Upload::Buffer { id, data, .. } = upload else {
            return false;
        };
        *id == instances
            && data
                .as_chunks::<{ std::mem::size_of::<lumen_text::GpuGlyphInstance>() }>()
                .0
                .iter()
                .map(|bytes| bytemuck::pod_read_unaligned::<lumen_text::GpuGlyphInstance>(bytes))
                .any(|glyph| glyph.rect[2] > 2500.0 && glyph.rect[3] > 2500.0)
    });
    assert!(has_huge_glyph, "the large glyph is bound at its own size");
    let enlarged = render(&mut renderer, &huge, 0.0, &media);
    assert!(drawn(&enlarged.rgba));
    assert!(
        enlarged.rgba != initial.rgba,
        "the enlarged glyph changes the frame"
    );
    assert!(
        enlarged.rgba == render(&mut gpu.renderer(), &huge, 0.0, &media).rgba,
        "the reused atlas draws what a new one does"
    );

    renderer.release_text_atlas();
    assert!(renderer.compiled().is_none());
    let rebound = renderer.bind_frame(&huge, 0, &media).unwrap();
    assert!(
        rebound.texture_upload_count() > 0,
        "the glyphs are uploaded again"
    );
    renderer.upload_bound_frame(&rebound).unwrap();
    assert!(
        render(&mut renderer, &huge, 0.0, &media).rgba == enlarged.rgba,
        "released and rebuilt glyphs draw the frame they did"
    );
}

#[test]
fn glyphs_beyond_the_atlas_are_not_uploaded_again_on_every_bind() {
    const CONTENT: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let gpu = gpu();
    let media = TestMedia::default();
    let mut renderer = gpu.renderer();
    let text = |content: &str, font_size: u32| -> Composition {
        one_text(json!({ "content": content, "font_size": font_size })).into()
    };

    let overflowing = text(CONTENT, 1800);
    let first = renderer.bind_frame(&overflowing, 0, &media).unwrap();
    let placed = first.texture_upload_count();
    assert!(
        (1..CONTENT.len()).contains(&placed),
        "{placed} of {} glyphs fit, so the layout overflows",
        CONTENT.len()
    );
    renderer.upload_bound_frame(&first).unwrap();
    for frame in 1..4 {
        let repeated = renderer.bind_frame(&overflowing, frame, &media).unwrap();
        assert_eq!(repeated.texture_upload_count(), 0, "frame {frame}");
        renderer.upload_bound_frame(&repeated).unwrap();
    }

    // A glyph too big for the atlas is drawn from one distance field, uploaded once.
    let oversized = text("W", 20000);
    renderer.release_resources();
    for frame in 4..7 {
        let bound = renderer.bind_frame(&oversized, frame, &media).unwrap();
        assert_eq!(
            bound.texture_upload_count(),
            usize::from(frame == 4),
            "frame {frame}"
        );
        renderer.upload_bound_frame(&bound).unwrap();
    }

    let recovered = text("W", 72);
    renderer.release_resources();
    let bound = renderer.bind_frame(&recovered, 7, &media).unwrap();
    assert!(bound.texture_upload_count() > 0);
    renderer.upload_bound_frame(&bound).unwrap();
    let pixels = render(&mut renderer, &recovered, 7.0, &media).rgba;
    assert!(drawn(&pixels), "the atlas draws again once glyphs fit");
    assert!(
        pixels == render(&mut gpu.renderer(), &recovered, 7.0, &media).rgba,
        "the atlas draws what a new one does"
    );
}

fn atlas_writes(
    renderer: &GpuCompositionRenderer,
    bound: &BoundFrame,
) -> Vec<([u32; 2], [u32; 2])> {
    let atlas = renderer
        .compiled()
        .unwrap()
        .plan
        .textures()
        .iter()
        .find(|texture| texture.label.as_deref() == Some("text:shared-atlas"))
        .unwrap()
        .id;
    bound
        .frame_update()
        .uploads()
        .iter()
        .filter_map(|upload| match upload {
            Upload::TextureRgba8Region {
                id, origin, size, ..
            } if *id == atlas => Some(([origin[0], origin[1]], [size.width, size.height])),
            _ => None,
        })
        .collect()
}

fn overlap(
    (at, size): &([u32; 2], [u32; 2]),
    (other_at, other_size): &([u32; 2], [u32; 2]),
) -> bool {
    (0..2).all(|axis| {
        at[axis] < other_at[axis] + other_size[axis] && other_at[axis] < at[axis] + size[axis]
    })
}

/// Binds frame 0, then frame 1, uploads frame 1 and then tries frame 0's upload.
struct StaleUpload {
    scene: Scene,
    /// Whether frame 1 needs a plan frame 0 did not.
    new_plan: bool,
    /// Whether the glyphs of the two frames are written over each other in the atlas.
    shared_slot: bool,
}

impl StaleUpload {
    fn run(self) {
        let gpu = gpu();
        let media = TestMedia::default();
        let composition: Composition = self.scene.into();
        let frame_0 = render(&mut gpu.renderer(), &composition, 0.0, &media).rgba;
        let fresh = render(&mut gpu.renderer(), &composition, 1.0, &media).rgba;
        assert!(drawn(&fresh), "nothing is drawn");
        assert!(fresh != frame_0, "the frames look alike");

        let mut renderer = gpu.renderer();
        let older = renderer.bind_frame(&composition, 0, &media).unwrap();
        let compiles = renderer.plan_compile_count();
        let current = renderer.bind_frame(&composition, 1, &media).unwrap();
        assert_eq!(
            renderer.plan_compile_count() > compiles,
            self.new_plan,
            "frame 1 compiles a plan of its own"
        );
        assert!(older.texture_upload_count() > 0 && current.texture_upload_count() > 0);
        if self.shared_slot {
            let (older, current) = (
                atlas_writes(&renderer, &older),
                atlas_writes(&renderer, &current),
            );
            assert!(
                older.iter().any(|a| current.iter().any(|b| overlap(a, b))),
                "frame 1 writes {current:?}, clear of frame 0's {older:?}"
            );
        }

        renderer.upload_bound_frame(&current).unwrap();
        let error = renderer.upload_bound_frame(&older).unwrap_err();
        assert!(
            error.to_string().contains("stale bound text frame"),
            "{error}"
        );

        let rebound = renderer.bind_frame(&composition, 1, &media).unwrap();
        assert_eq!(rebound.texture_upload_count(), 0);
        renderer.submit_bound_frame(&rebound).unwrap();
        assert!(
            render(&mut renderer, &composition, 1.0, &media).rgba == fresh,
            "the rejected upload left the atlas as frame 1 needs it"
        );
    }
}

#[test]
fn a_bound_frame_is_not_uploaded_once_a_later_one_is_bound() {
    let flip = "=if(frame == 0, \"W\", \"M\")";
    let blank = json!({ "paint": [0, 0, 0, 0], "width": 8, "height": 4 });
    let rows = vec![
        (
            "same plan under atlas pressure",
            StaleUpload {
                scene: scene(
                    (512, 512),
                    json!([text_node(
                        1,
                        json!({
                            "content": flip, "font_size": 3000, "position": [0, -600]
                        })
                    )]),
                )
                .frames(12),
                new_plan: false,
                shared_slot: true,
            },
        ),
        (
            "frame 1 switches to another plan",
            StaleUpload {
                scene: scene(
                    (64, 64),
                    json!([
                        text_node(1, json!({ "content": flip, "font_size": 32 })),
                        { "id": 2, "type": "background", "params": blank },
                        { "id": 3, "type": "background", "params": blank },
                        { "id": 4, "type": "switch", "params": { "selected_layer": "=frame" },
                          "in": { "layers": [2, 3] } },
                        { "id": 5, "type": "merge", "in": { "base": 1, "overlay": 4 } },
                    ]),
                )
                .frames(12),
                new_plan: true,
                shared_slot: false,
            },
        ),
    ];
    check_each(rows, StaleUpload::run);
}

fn text_domain_is_bounded(renderer: &GpuCompositionRenderer) -> bool {
    let text = PortRef::new(NodeId::new(1), "output".to_string());
    match &renderer.compiled().expect("a frame rendered").node_outputs[&text] {
        CompiledOutput::Raster(raster) => !raster.domain.is_full_frame(),
        CompiledOutput::Empty => panic!("the text is empty"),
    }
}

/// Text node 1, `content` in `family` at size 20.
fn small_text(content: &str, family: &str, line_height: f64, position: [u32; 2]) -> Value {
    text_node(
        1,
        json!({
            "content": content, "font_family": family, "font_size": 20,
            "line_height": line_height, "position": position
        }),
    )
}

/// Prepares the renderer, and returns the media its measured render goes through.
type Before = fn(&Gpu, &mut GpuCompositionRenderer, &Composition, &TestMedia) -> TestMedia;

/// Text in a font that arrives after its bounds were measured must be drawn in full.
struct FontBounds {
    scene: Scene,
    media: TestMedia,
    /// Runs on the renderer before its measured render.
    before: Before,
}

impl FontBounds {
    fn run(self) {
        let gpu = gpu();
        let composition: Composition = self.scene.into();
        let fallback = render(
            &mut gpu.renderer(),
            &composition,
            0.0,
            &self.media.without_fonts(),
        );
        let mut renderer = gpu.renderer();
        let media = (self.before)(&gpu, &mut renderer, &composition, &self.media);
        let in_font = render(&mut renderer, &composition, 0.0, &media);
        assert!(
            in_font.rgba != fallback.rgba,
            "the font does not change the text"
        );
        assert!(drawn(&in_font.rgba), "nothing is drawn");
        assert!(
            text_domain_is_bounded(&renderer),
            "the text fills the canvas"
        );
        let reference = render(&mut gpu.reference(), &composition, 0.0, &self.media);
        if let Err(error) = within_rounding(&in_font.rgba, &reference.rgba) {
            panic!("the text is clipped to bounds measured before its font: {error}");
        }
    }
}

#[test]
fn text_bounds_hold_the_ink_of_a_font_loaded_after_they_were_measured() {
    // `served` is the family the media names the font by, and `internal` the family inside it.
    // Text is bounded only in a family its media names or in the default one, so the last two rows
    // widen "Roboto" itself, in a text session no other test reads.
    let font = |served: &str, internal: &str, scale| {
        let id = format!("{served}:{scale}");
        TestMedia::default().font(served, &id, roboto_scaled(internal, scale))
    };
    let wide = |family: &str| {
        scene(
            (256, 192),
            json!([small_text("WWWWWWWWWW", family, 1.2, [5, 40])]),
        )
    };
    let rows = vec![
        (
            "compiled without media",
            FontBounds {
                scene: scene(
                    (640, 360),
                    json!([small_text("WWW", "WideIn", 1.2, [10, 90])]),
                ),
                media: font("WideIn", "WideIn", 2),
                before: |_, renderer, composition, media| {
                    renderer.compile(composition).unwrap();
                    media.clone()
                },
            },
        ),
        (
            "ink eight times the em tall",
            FontBounds {
                scene: scene(
                    (640, 360),
                    json!([small_text("WWW", "TallIn", 1.0, [10, 90])]),
                ),
                media: font("TallIn", "TallIn", 8),
                before: |_, renderer, composition, media| {
                    let format = wgpu::TextureFormat::Rgba8Unorm;
                    renderer
                        .compile_with_media(composition, media, format)
                        .unwrap();
                    media.clone()
                },
            },
        ),
        (
            // Only the merge, evaluated once the frame binds, measures in the family.
            "loaded while the frame binds",
            FontBounds {
                scene: scene(
                    (256, 192),
                    json!([
                        small_text("WWWWWWWWWW", "Roboto", 1.2, [5, 40]),
                        text_node(2, json!({ "content": "W", "position": [0, 400] })),
                        { "id": 3, "type": "merge", "in": { "base": 1, "overlay": 2 }, "params": {
                            "opacity": "=min(text_width(\"W\", 32, 0, \"Loader\"), 1)"
                        } },
                    ]),
                ),
                media: font("Loader", "Roboto", 2),
                before: |_, _, _, media| media.clone(),
            },
        ),
        (
            "loaded by another renderer",
            FontBounds {
                scene: wide("Roboto"),
                media: font("Remote", "Roboto", 2),
                before: |gpu, renderer, composition, media| {
                    // The renderer's media never serves the font, so only the font generation
                    // tells it that another renderer loaded one.
                    let unloaded = media.without_fonts();
                    render(renderer, composition, 0.0, &unloaded);
                    let loader = scene((8, 8), json!([small_text("W", "Remote", 1.2, [0, 0])]));
                    render(&mut gpu.renderer(), &loader.into(), 0.0, media);
                    unloaded
                },
            },
        ),
    ];
    check_each(rows, FontBounds::run);
}

/// Text whose font arrives after its output was kept must be drawn again.
struct FontArrival {
    scene: Scene,
    media: TestMedia,
    /// Frames rendered before the media's fonts are served.
    before: Vec<f64>,
    after: f64,
}

impl FontArrival {
    fn run(self) {
        let gpu = gpu();
        let composition: Composition = self.scene.into();
        let unloaded = self.media.without_fonts();
        let fallback = render(&mut gpu.renderer(), &composition, self.after, &unloaded);
        let mut renderer = gpu.renderer();
        for &frame in &self.before {
            render(&mut renderer, &composition, frame, &unloaded);
        }
        let loaded = render(&mut renderer, &composition, self.after, &self.media);
        assert!(
            loaded.rgba != fallback.rgba,
            "the text keeps the fallback font"
        );
        let reference = render(&mut gpu.reference(), &composition, self.after, &self.media);
        if let Err(error) = within_rounding(&loaded.rgba, &reference.rgba) {
            panic!("differs from a render that never held the text: {error}");
        }
    }
}

#[test]
fn kept_text_is_drawn_again_once_its_font_arrives() {
    let arriving = |family: &str| {
        TestMedia::default().font(family, &format!("{family}:2"), roboto_scaled(family, 2))
    };
    let memoized = |content: Value| {
        scene(
            (128, 64),
            json!([
                text_node(1, json!({ "content": content, "font_size": 20, "position": [10, 10] })),
                { "id": 2, "type": "memo", "params": { "key": "=frame * 0" }, "in": { "source": 1 } },
            ]),
        )
    };
    let rows = vec![
        (
            "static node family, kept under a moving square",
            FontArrival {
                scene: scene(
                    (128, 64),
                    json!([
                        small_text("WWW", "Cachey", 1.2, [10, 10]),
                        { "id": 2, "type": "shape", "params": {
                            "position": "=vec2(frame * 4, 0)", "width": 4, "height": 4,
                            "fill_paint": [255, 255, 255, 255]
                        } },
                        { "id": 3, "type": "raster_multimerge", "in": { "layers": [1, 2] } },
                    ]),
                ),
                media: arriving("Cachey"),
                before: vec![0.0, 1.0],
                after: 2.0,
            },
        ),
        (
            "memo, literal span family",
            FontArrival {
                scene: memoized(json!([{ "text": "WWW", "font_family": "Spanny" }])),
                media: arriving("Spanny"),
                before: vec![0.0, 1.0],
                after: 2.0,
            },
        ),
        (
            "memo, expression span family",
            FontArrival {
                scene: memoized(json!([{ "text": "WWW", "font_family": "=\"Dyfont\"" }])),
                media: arriving("Dyfont"),
                before: vec![0.0],
                after: 1.0,
            },
        ),
    ];
    check_each(rows, FontArrival::run);
}

#[test]
fn a_memo_holds_text_whose_family_would_fail_to_evaluate() {
    let gpu = gpu();
    let composition: Composition = scene(
        (128, 64),
        json!([
            text_node(1, json!({
                "content": "WWW", "font_size": 20, "position": [10, 10],
                "font_family": "=if(frame == 0, \"Roboto\", node(999, \"font_family\"))"
            })),
            { "id": 2, "type": "memo", "params": { "key": "=frame * 0" }, "in": { "source": 1 } },
        ]),
    )
    .into();
    let media = TestMedia::default();
    let mut renderer = gpu.renderer();
    let first = render(&mut renderer, &composition, 0.0, &media);
    assert!(drawn(&first.rgba));
    assert!(
        render(&mut renderer, &composition, 1.0, &media).rgba == first.rgba,
        "the memo draws a frame other than the one it holds"
    );
}

#[test]
fn measuring_text_through_a_time_remap_loads_the_family_it_names_once_per_id() {
    let font = roboto_scaled("RemapX", 2);
    let media = TestMedia::default().font("RemapX", "remap-x:1", font.clone());
    let composition: Composition = scene(
        (8, 4),
        json!([
            text_node(1, json!({
                "content": "WWW",
                "font_family": "=if(frame == 1, \"RemapX\", \"Roboto\")",
                "font_size": 32,
                "position": "=vec2(text_width(node(1)), text_height(node(1)))"
            })),
            { "id": 2, "type": "time_remap", "params": { "frame": 1 }, "in": { "source": 1 } },
        ]),
    )
    .into();

    let mut request = lumen_text::TextLayoutRequest::new("WWW");
    request.spans[0].font_family = "RemapX".to_string();
    request.spans[0].font_size = 32.0;
    let mut reference = lumen_text::TextSystem::new();
    let fallback_width = reference.measure(&request).width;
    reference.load_font_data(font);
    let expected_width = reference.measure(&request).width;
    assert!(expected_width > fallback_width * 1.5);

    let compile = |media: &TestMedia| {
        CompileContext::with_media(&composition, media, wgpu::TextureFormat::Rgba8Unorm)
            .compile()
            .unwrap()
    };
    let compiled = compile(&media);
    let instances = compiled
        .plan
        .buffers()
        .iter()
        .find(|buffer| buffer.label.as_deref() == Some("text:1:instances"))
        .unwrap()
        .id;
    let first_rect = |media: &TestMedia| -> [f32; 4] {
        let bound = FrameBindContext::with_media(&composition, 0, media)
            .bind(&compiled)
            .unwrap();
        bound
            .frame_update()
            .uploads()
            .iter()
            .find_map(|upload| match upload {
                Upload::Buffer { id, data, .. } if *id == instances => {
                    Some(bytemuck::pod_read_unaligned(&data[..16]))
                }
                _ => None,
            })
            .unwrap()
    };
    let rect = first_rect(&media);
    assert_eq!(first_rect(&media), rect);
    assert!(
        (rect[0] - expected_width).abs() < 3.0,
        "{rect:?} for {expected_width}"
    );
    assert_eq!(media.font_requests(), 1);

    let replaced = media
        .clone()
        .font("RemapX", "remap-x:2", roboto_scaled("RemapX", 2));
    assert_eq!(first_rect(&replaced), rect);
    assert_eq!(media.font_requests(), 2);
}

const ROBOTO: &[u8] = lumen_text::ROBOTO_REGULAR_BYTES;
const INTER: &[u8] = include_bytes!("../../../text/assets/inter/InterVariable-Latin.ttf");

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

/// A text media that serves `family` under no font file, so its session is its own.
fn private_session() -> TestMedia {
    TestMedia::default().font("Absent", "job-font-absent", Vec::new())
}

#[test]
fn a_family_only_one_job_has_does_not_reach_another_jobs_measurements() {
    let measure = |media: &TestMedia| {
        Expression::parse("text_width(\"WWW\", 32, 0, \"OnlyIn\")")
            .unwrap()
            .evaluate(&ExpressionContext {
                media: Some(media),
                ..Default::default()
            })
            .unwrap()
            .as_f64()
            .unwrap()
    };
    let with_font =
        TestMedia::default().font("OnlyIn", "job-font-only-in-one", roboto_scaled("OnlyIn", 2));
    let without = private_session();

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
    let gpu = gpu();
    // What is drawn and where both depend on the font: measuring must read the job's too.
    let composition = |family: &str| -> Composition {
        one_text(json!({
            "content": "Rag Wave 28", "font_family": family, "font_size": 64,
            "position": format!("=vec2(text_width(\"WWWWWW\", 32, 0, \"{family}\"), 40)")
        }))
        .into()
    };
    let jobs = || {
        [
            TestMedia::default().font("Face", "job-font-roboto", font_named(ROBOTO, "Face")),
            TestMedia::default().font("Face", "job-font-inter", font_named(INTER, "Face")),
        ]
    };
    let shared_family = composition("Face");
    let mut renderer = gpu.renderer();

    // The true faces under their own names, which is how each file must draw under the shared one.
    let roboto = render(
        &mut renderer,
        &composition("Roboto"),
        0.0,
        &TestMedia::default(),
    )
    .rgba;
    let inter_job = TestMedia::default().font("Inter", "job-font-true-inter", INTER.to_vec());
    let inter = render(&mut gpu.renderer(), &composition("Inter"), 0.0, &inter_job).rgba;
    assert!(drawn(&roboto));
    assert_ne!(roboto, inter);
    let expected = [&roboto, &inter];

    // In turns on one renderer, whose plans and atlas outlive each store's frame.
    let turns = jobs();
    for job in [0, 1, 0, 1, 1, 0] {
        let pixels = render(&mut renderer, &shared_family, 0.0, &turns[job]).rgba;
        assert!(pixels == *expected[job], "job {job} drew another's font");
    }

    // Files alike in all but size number their glyphs alike, so their atlas keys match.
    let sized = |scale: u16| {
        TestMedia::default().font(
            "Sizing",
            &format!("job-font-sized-{scale}"),
            roboto_scaled("Sizing", scale),
        )
    };
    let glyphs: Composition = one_text(json!({
        "content": "W", "font_family": "Sizing", "font_size": 64, "position": [16, 40]
    }))
    .into();
    let alone = [1, 2].map(|scale| render(&mut gpu.renderer(), &glyphs, 0.0, &sized(scale)).rgba);
    assert_ne!(alone[0], alone[1]);
    let (large, larger) = (sized(1), sized(2));
    for (job, expected) in [
        (&large, &alone[0]),
        (&larger, &alone[1]),
        (&large, &alone[0]),
    ] {
        let pixels = render(&mut renderer, &glyphs, 0.0, job).rgba;
        assert!(pixels == *expected, "another job's glyphs were drawn");
    }

    // At once, each job on a renderer of its own as `lumen-server` runs them.
    let together = jobs();
    let renderers = [gpu.renderer(), gpu.renderer()];
    std::thread::scope(|scope| {
        for ((job, expected), mut renderer) in together.iter().zip(expected).zip(renderers) {
            let shared_family = &shared_family;
            scope.spawn(move || {
                for _ in 0..8 {
                    let pixels = render(&mut renderer, shared_family, 0.0, job).rgba;
                    assert!(pixels == *expected, "a job drew another's font");
                }
            });
        }
    });
}

#[test]
fn replacing_a_font_mid_job_leaves_no_stale_glyph_on_screen() {
    let gpu = gpu();
    let glyph: Composition = one_text(json!({
        "content": "W", "font_family": "Sizing", "font_size": 64, "position": [16, 40]
    }))
    .into();
    let data = |scale: u16| roboto_scaled("Sizing", scale);
    let alone = [1, 2].map(|scale| {
        let job = TestMedia::default().font("Sizing", &format!("alone-{scale}"), data(scale));
        render(&mut gpu.renderer(), &glyph, 0.0, &job).rgba
    });
    assert_ne!(alone[0], alone[1]);

    // Rebuilt fonts number their faces alike and these their glyphs alike, so only the rebuild
    // tells one font's atlas keys from the other's. The clones keep the job's text session.
    let mut renderer = gpu.renderer();
    let job = private_session();
    for (turn, scale) in [1, 2, 1].into_iter().enumerate() {
        let media = job
            .clone()
            .font("Sizing", &format!("turn-{turn}"), data(scale));
        let pixels = render(&mut renderer, &glyph, 0.0, &media).rgba;
        assert!(pixels == alone[usize::from(scale) - 1], "turn {turn}");
    }

    // Only the merge asks for this font, once the text has placed its glyphs. It goes by the name
    // the text draws in, and being the latest load it takes over the text.
    let both: Composition = scene(
        (256, 128),
        json!([
            text_node(1, json!({
                "content": "W", "font_family": "Sizing", "font_size": 64, "position": [16, 40]
            })),
            text_node(2, json!({
                "content": "W", "font_family": "Sizing", "font_size": 64, "position": [128, 40]
            })),
            { "id": 3, "type": "merge", "in": { "base": 1, "overlay": 2 }, "params": {
                "opacity": "=min(text_width(\"W\", 32, 0, \"Second\"), 1)"
            } },
        ]),
    )
    .into();
    let job = TestMedia::default()
        .font("Sizing", "both-sizing", data(1))
        .font("Second", "both-second", data(2));
    let mut renderer = gpu.renderer();
    let first = render(&mut renderer, &both, 0.0, &job).rgba;
    let settled = render(&mut renderer, &both, 0.0, &job).rgba;
    assert!(first == settled);
    assert!(drawn(&first));
}

/// Renders `nodes` on a 128 × 64 canvas, which must not fail and must draw something.
fn drawn_in(gpu: &Gpu, nodes: Value, media: &TestMedia) {
    let composition: Composition = scene((128, 64), nodes).into();
    let pixels = render(&mut gpu.renderer(), &composition, 0.0, media).rgba;
    assert!(drawn(&pixels));
}

#[test]
fn a_font_loaded_while_binding_resizes_constant_text_instead_of_failing() {
    let gpu = gpu();
    // The first text measures a node whose font loads as the frame binds. That font, unlike the
    // bundled one, has no "ffi" ligature, so the second text, sized for one glyph, needs three.
    let media = TestMedia::default()
        .font("Loader", "late-font", font_named(INTER, "Qzxvj"))
        .font("Qzxvj", "no-font", Vec::new());
    drawn_in(
        &gpu,
        json!([
            text_node(99, json!({ "content": "x", "font_family": "Loader" })),
            text_node(1, json!({
                "content": "a",
                "glyph_offset": "=vec2(0 * text_width(node(99)), 0)"
            })),
            text_node(2, json!({
                "content": "ffi", "font_family": "Qzxvj", "font_size": 40, "position": [4, 10]
            })),
            { "id": 3, "type": "raster_multimerge", "in": { "layers": [1, 2] } },
        ]),
        &media,
    );
}

#[test]
fn many_fonts_loaded_while_binding_settle_in_one_render() {
    let gpu = gpu();
    // Each pair is a text measuring a node whose font loads as the frame binds, then a text that
    // font outgrows, so every font stops a bind.
    let mut media = TestMedia::default();
    let mut nodes = Vec::new();
    let mut layers = Vec::new();
    for font in 0..5 {
        let (measured, measuring, outgrown) = (200 + font, 2 * font + 1, 2 * font + 2);
        media = media
            .font(
                &format!("Load{font}"),
                &format!("Load{font}"),
                font_named(INTER, &format!("Late{font}")),
            )
            .font(&format!("Late{font}"), &format!("Late{font}"), Vec::new());
        nodes.extend([
            text_node(
                measured,
                json!({
                    "content": "x", "font_family": format!("Load{font}")
                }),
            ),
            text_node(
                measuring,
                json!({
                    "content": "a",
                    "glyph_offset": format!("=vec2(0 * text_width(node({measured})), 0)")
                }),
            ),
            text_node(
                outgrown,
                json!({
                    "content": "ffi", "font_family": format!("Late{font}"), "font_size": 40
                }),
            ),
        ]);
        layers.extend([measuring, outgrown]);
    }
    nodes.push(json!({ "id": 100, "type": "raster_multimerge", "in": { "layers": layers } }));
    drawn_in(&gpu, json!(nodes), &media);
}

/// Gives "Churn" a resolver with a new id each time it is asked, so its font never settles.
#[derive(Debug, Default)]
struct ChurningFont {
    asked: AtomicU32,
    session: TextSession,
}

struct ChurningResolver(String);

impl FontResolver for ChurningResolver {
    fn id(&self) -> &str {
        &self.0
    }

    fn data(&self) -> Result<Vec<Vec<u8>>, MediaError> {
        Ok(vec![font_named(INTER, "Churn")])
    }
}

impl MediaStore for ChurningFont {
    fn font_resolver_id(&self, family: &str) -> Option<String> {
        (family == "Churn").then(|| format!("churn-{}", self.asked.fetch_add(1, Ordering::Relaxed)))
    }

    fn get_font_resolver(&self, family: &str) -> Option<Box<dyn FontResolver>> {
        let id = self.font_resolver_id(family)?;
        Some(Box::new(ChurningResolver(id)))
    }

    fn text_session(&self) -> Option<&TextSession> {
        Some(&self.session)
    }
}

#[test]
fn a_font_that_never_settles_fails_the_render() {
    let gpu = gpu();
    let composition: Composition = scene(
        (64, 64),
        json!([text_node(
            1,
            json!({ "content": "x", "font_family": "Churn" })
        )]),
    )
    .into();
    let error = gpu
        .renderer()
        .render_frame_at(&composition, 0.0, &ChurningFont::default())
        .expect_err("the font loads again on every bind")
        .to_string();
    assert!(error.contains("fonts kept loading"), "{error}");
}

#[test]
fn text_past_the_instance_limit_is_a_render_error_that_counts_drawn_glyphs() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let media = TestMedia::default();
    // The animated size gives the text the full limit, and the constant one a limit of its own.
    for font_size in [json!(8), json!("=8 + frame * 0")] {
        let composition = |content: String| -> Composition {
            scene(
                (64, 64),
                json!([text_node(
                    1,
                    json!({ "content": content, "font_size": font_size })
                )]),
            )
            .into()
        };
        // Whitespace is not drawn, so it does not count toward the limit.
        let spaced = composition(format!("ab{}", " ".repeat(3000)));
        assert!(drawn(&render(&mut renderer, &spaced, 0.0, &media).rgba));

        let long = composition("a".repeat(2049));
        let error = renderer
            .render_frame_at(&long, 0.0, &media)
            .expect_err("2049 glyphs exceed the limit")
            .to_string();
        assert!(
            error.contains("Text node 1") && error.contains("2049") && error.contains("2048"),
            "{font_size}: {error}"
        );
    }
}

#[test]
fn text_dropped_by_a_full_atlas_still_counts_toward_the_instance_limit() {
    let gpu = gpu();
    // Glyphs this large overflow the atlas, which drops some of them.
    let composition: Composition = scene(
        (64, 64),
        json!([text_node(
            1,
            json!({
                "content": "ABCDEFGHIJKLMNOPQRSTUVWXYZ".repeat(80),
                "font_size": "=1500 + frame * 0", "render_mode": "raster"
            })
        )]),
    )
    .into();
    let error = gpu
        .renderer()
        .render_frame_at(&composition, 0.0, &TestMedia::default())
        .expect_err("2080 glyphs exceed the limit")
        .to_string();
    assert!(
        error.contains("Text node 1") && error.contains("2080") && error.contains("2048"),
        "{error}"
    );
}
