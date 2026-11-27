use std::collections::HashSet;

use super::*;
use crate::msdf::{MSDF_EM_SIZE, MSDF_MAX_EM_SIZE, msdf_em_size};
use crate::tests::scaled_roboto;
use crate::{DEFAULT_FONT_FAMILY, GLYPH_RANGE_MASK, MSDF_PX_RANGE, TextAlign, TextLayoutRequest};
use cosmic_text::CacheKeyFlags;

#[derive(Debug, Clone)]
struct AtlasRender {
    pixels: Vec<u8>,
    instances: Vec<GpuGlyphInstance>,
}

impl TextSystem {
    /// Draws `layout` into a fresh `size`² atlas and returns its pixels.
    fn render_atlas(
        &mut self,
        layout: &TextLayout,
        mode: TextRenderMode,
        size: u32,
    ) -> AtlasRender {
        let mut atlas = GlyphAtlas::new(size);
        atlas.begin_frame();
        let prepared = self.prepare_glyphs(&mut atlas, layout, mode, &[], &mut None);
        let mut pixels = vec![0_u8; size as usize * size as usize * 4];
        for upload in &prepared.uploads {
            let row_len = upload.size[0] as usize * 4;
            for row in 0..upload.size[1] as usize {
                let dst = ((upload.origin[1] as usize + row) * size as usize
                    + upload.origin[0] as usize)
                    * 4;
                pixels[dst..dst + row_len]
                    .copy_from_slice(&upload.pixels[row * row_len..(row + 1) * row_len]);
            }
        }
        AtlasRender {
            pixels,
            instances: prepared.instances,
        }
    }
}

fn prepare(
    system: &mut TextSystem,
    atlas: &mut GlyphAtlas,
    layout: &TextLayout,
    mode: TextRenderMode,
) -> PreparedGlyphs {
    system.prepare_glyphs(atlas, layout, mode, &[], &mut None)
}

#[test]
fn an_atlas_keeps_no_glyphs_of_another_text_system() {
    let mut systems = [2, 4].map(|scale| {
        let mut system = TextSystem::new();
        system
            .load_family(DEFAULT_FONT_FAMILY, "scaled", vec![scaled_roboto(scale)])
            .unwrap();
        system
    });
    // Text shaped under one generation must not pass for another system's.
    assert_ne!(systems[0].font_generation(), systems[1].font_generation());
    let request = TextLayoutRequest::new("W");
    let [first, mut second] = systems
        .each_mut()
        .map(|system| system.shape(&request).place([0.0; 2]));
    // Each text system numbers its fonts alike, so only the baseline's bin differs.
    assert_eq!(
        first.glyphs[0].key.0.font_id,
        second.glyphs[0].key.0.font_id
    );
    second.glyphs[0].key = first.glyphs[0].key;

    let mut atlas = GlyphAtlas::new(1024);
    atlas.begin_frame();
    let [system_first, system_second] = &mut systems;
    let prepared = prepare(system_first, &mut atlas, &first, TextRenderMode::Raster);
    assert_eq!(prepared.uploads.len(), 1);
    atlas.commit(atlas.frame);
    atlas.begin_frame();
    let again = prepare(system_first, &mut atlas, &first, TextRenderMode::Raster);
    assert!(again.uploads.is_empty());

    atlas.begin_frame();
    let other = prepare(system_second, &mut atlas, &second, TextRenderMode::Raster);
    assert_eq!(other.uploads.len(), 1);
    assert_ne!(other.instances[0].rect[2], prepared.instances[0].rect[2]);
}

#[test]
fn glyphs_of_replaced_fonts_are_not_drawn_again() {
    let request = TextLayoutRequest::new("W");
    let mut system = TextSystem::new();
    system
        .load_family(DEFAULT_FONT_FAMILY, "half", vec![scaled_roboto(2)])
        .unwrap();
    let old = system.shape(&request).place([0.0; 2]);
    let shaped = system.shape(&request);
    let old_ink = system.ink_bounds(&shaped).unwrap();
    let mut atlas = GlyphAtlas::new(1024);
    atlas.begin_frame();
    let prepared = prepare(&mut system, &mut atlas, &old, TextRenderMode::Raster);
    atlas.commit(atlas.frame);

    system
        .load_family(DEFAULT_FONT_FAMILY, "quarter", vec![scaled_roboto(4)])
        .unwrap();
    let mut new = system.shape(&request).place([0.0; 2]);
    // A rebuild numbers the fonts alike, so only the baseline's bin differs here.
    assert_eq!(old.glyphs[0].key.0.font_id, new.glyphs[0].key.0.font_id);
    assert_eq!(old.glyphs[0].key.0.glyph_id, new.glyphs[0].key.0.glyph_id);
    new.glyphs[0].key = old.glyphs[0].key;

    let shaped = system.shape(&request);
    let new_ink = system.ink_bounds(&shaped).unwrap();
    assert!(new_ink[2] < old_ink[2]);
    atlas.begin_frame();
    let redrawn = prepare(&mut system, &mut atlas, &new, TextRenderMode::Raster);
    assert_eq!(redrawn.uploads.len(), 1);
    assert!(redrawn.instances[0].rect[2] < prepared.instances[0].rect[2] * 0.75);
}

#[test]
fn atlas_contains_glyph_pixels() {
    let mut system = TextSystem::new();
    let mut request = TextLayoutRequest::new("A");
    request.spans[0].font_size = 48.0;
    let layout = system.shape(&request).place([0.0; 2]);
    let render = system.render_atlas(&layout, TextRenderMode::Raster, 128);

    assert_eq!(render.instances.len(), 1);
    let instance = render.instances[0];
    assert!(instance.rect[2] > 0.0 && instance.uv_rect[2] > instance.uv_rect[0]);
    assert!(
        render
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[3] > 0)
    );
}

#[test]
fn colour_glyphs_are_stored_premultiplied() {
    let mut image = RasterImage {
        placement: zeno::Placement {
            left: 0,
            top: 0,
            width: 2,
            height: 1,
        },
        content: RasterContent::Color,
        source: RasterSource::ColorBitmap {
            straight_alpha: true,
        },
        data: vec![255, 0, 0, 128, 0, 100, 200, 255],
    };
    let key = TextSystem::new()
        .shape(&TextLayoutRequest::new("A"))
        .place([0.0; 2])
        .glyphs[0]
        .key
        .0;

    let bitmap = raster_pixels(&image, key);
    image.source = RasterSource::ColorOutline;
    let outline = raster_pixels(&image, key);

    assert_eq!(bitmap.pixels, [128, 0, 0, 128, 0, 100, 200, 255]);
    assert_eq!(outline.pixels, image.data);
    assert_eq!(bitmap.mode, GLYPH_MODE_RASTER_COLOR);
}

#[test]
fn bitmap_glyphs_snap_to_the_nearest_pixel() {
    let mut system = TextSystem::new();
    let shaped = system.shape(&TextLayoutRequest::new("A"));
    let image = RasterImage {
        source: RasterSource::ColorBitmap {
            straight_alpha: true,
        },
        content: RasterContent::Color,
        placement: zeno::Placement {
            left: 1,
            top: 8,
            width: 2,
            height: 2,
        },
        data: vec![0; 2 * 2 * 4],
    };
    let mut atlas = GlyphAtlas::new(64);
    let (slot, origin) = atlas.allocate([2, 2]).unwrap();
    for start in [20.0, -21.0] {
        let mut previous = f32::NEG_INFINITY;
        for step in 0..=8 {
            let y = start + step as f32 * 0.125;
            let layout = shaped.place([3.3, y]);
            let glyph = &layout.glyphs[0];
            let pixels = raster_pixels(&image, glyph.key.0);
            let placed = PlacedGlyph {
                slot,
                origin,
                size: pixels.size,
                left: pixels.left,
                top: pixels.top,
                mode: pixels.mode,
            };
            let rendered_y = placed.instance(glyph, AtlasKey::Raster(glyph.key.0)).rect[1] + 8.0;
            let pen_y = glyph.position[1];
            // Half a pixel of snapping plus cosmic-text's quarter-pixel bins.
            assert!(
                (rendered_y - pen_y).abs() <= 0.625,
                "pen y {pen_y}, drawn at {rendered_y}"
            );
            assert!(rendered_y >= previous, "pen y {pen_y} moved backwards");
            previous = rendered_y;
        }
    }
}

#[test]
fn atlas_evicts_before_running_out_of_allocator_ids() {
    let mut system = TextSystem::new();
    let template = system
        .shape(&TextLayoutRequest::new("A"))
        .place([0.0; 2])
        .glyphs[0]
        .key
        .0;
    let mut atlas = GlyphAtlas::new(512);
    for index in 0..70_000_u32 {
        if index % 1000 == 0 {
            atlas.begin_frame();
        }
        let (slot, origin) = atlas.allocate([1, 1]).unwrap();
        let key = CacheKey {
            font_size_bits: index,
            ..template
        };
        atlas.insert(
            AtlasKey::Raster(key),
            AtlasImage::Placed(PlacedGlyph {
                slot,
                origin,
                size: [1, 1],
                left: 0.0,
                top: 0.0,
                mode: GLYPH_MODE_RASTER_MASK,
            }),
        );
    }
    assert_eq!(atlas.glyphs.len(), MAX_ATLAS_GLYPHS);
}

#[test]
fn raster_glyphs_are_placed_on_whole_pixels() {
    let mut system = TextSystem::new();
    let mut request = TextLayoutRequest::new("AV Sharp text\nمرحبا بالعالم wraps here");
    request.spans[0].font_size = 31.0;
    request.max_width = Some(220.0);
    request.align = TextAlign::Center;
    let shaped = system.shape(&request);

    let first = shaped.place([10.1, 5.3]);
    let second = shaped.place([10.6, 5.3]);
    let render = system.render_atlas(&second, TextRenderMode::Raster, 1024);

    assert_ne!(first.glyphs[0].key, second.glyphs[0].key);
    assert!(
        first
            .glyphs
            .iter()
            .chain(&second.glyphs)
            .all(|glyph| glyph.x.fract() == 0.0 && glyph.y.fract() == 0.0)
    );
    assert!(render.instances.len() > 10);
    assert!(
        render
            .instances
            .iter()
            .all(|instance| instance.rect[0].fract() == 0.0 && instance.rect[1].fract() == 0.0)
    );
}

#[test]
fn raster_ink_follows_fractional_y() {
    let mut system = TextSystem::new();
    let mut request = TextLayoutRequest::new("H");
    request.spans[0].font_size = 24.0;
    let shaped = system.shape(&request);
    let size = 64;
    let ink_center = |system: &mut TextSystem, y: f32| {
        let layout = shaped.place([4.0, y]);
        let render = system.render_atlas(&layout, TextRenderMode::Raster, size);
        let instance = render.instances[0];
        let [left, top] = [instance.uv_rect[0] as usize, instance.uv_rect[1] as usize];
        let (mut weight, mut sum) = (0.0, 0.0);
        for row in 0..instance.rect[3] as usize {
            for col in 0..instance.rect[2] as usize {
                let index = ((top + row) * size as usize + left + col) * 4 + 3;
                let alpha = f32::from(render.pixels[index]);
                weight += alpha;
                sum += alpha * (instance.rect[1] + row as f32 + 0.5);
            }
        }
        sum / weight
    };

    let bin = |y: f32| shaped.place([4.0, y]).glyphs[0].key.0.y_bin;
    assert_ne!(bin(20.0), bin(20.5));
    let base = ink_center(&mut system, 20.0);
    for step in [0.25, 0.5, 0.75, 1.0, -0.25, -0.75] {
        let moved = ink_center(&mut system, 20.0 + step) - base;
        assert!(
            (moved - step).abs() < 0.15,
            "origin moved {step}px, ink moved {moved}px"
        );
    }
}

#[test]
fn overflowed_glyph_recovers_when_working_set_shrinks() {
    let mut system = TextSystem::new();
    let mut atlas = GlyphAtlas::new(64);
    let mut overfull = None;
    let mut request = TextLayoutRequest::new("ABCDEFGHIJKLMNOPQRSTUVWXYZ");
    request.spans[0].font_size = 40.0;
    let mut layout = system.shape(&request).place([0.0; 2]);
    atlas.begin_frame();
    let full = system.prepare_glyphs(
        &mut atlas,
        &layout,
        TextRenderMode::Raster,
        &[],
        &mut overfull,
    );
    atlas.commit(atlas.frame);
    assert!(full.dropped_glyphs > 0);
    let dropped = layout
        .glyphs
        .iter()
        .find(|glyph| !atlas.glyphs.contains_key(&AtlasKey::Raster(glyph.key.0)))
        .unwrap()
        .clone();
    layout.glyphs = vec![dropped];

    atlas.begin_frame();
    let recovered = system.prepare_glyphs(
        &mut atlas,
        &layout,
        TextRenderMode::Raster,
        &[],
        &mut overfull,
    );
    assert_eq!(recovered.dropped_glyphs, 0);
    assert!(!recovered.instances.is_empty());
    assert!(!recovered.uploads.is_empty());
}

#[test]
fn whitespace_the_full_atlas_never_reached_is_not_dropped() {
    let mut system = TextSystem::new();
    let mut dropped = |text: &str| {
        let mut request = TextLayoutRequest::new(text);
        request.spans[0].font_size = 40.0;
        let layout = system.shape(&request).place([0.0; 2]);
        let mut atlas = GlyphAtlas::new(64);
        atlas.begin_frame();
        prepare(&mut system, &mut atlas, &layout, TextRenderMode::Raster).dropped_glyphs
    };
    let letters = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let without_spaces = dropped(letters);
    assert!(without_spaces > 0);
    assert_eq!(
        dropped(&format!("{letters}{}", " ".repeat(100))),
        without_spaces
    );
}

#[test]
fn overflowed_layout_recovers_when_other_layout_becomes_stale() {
    let mut system = TextSystem::new();
    let mut atlas = GlyphAtlas::new(64);
    let mut first = TextLayoutRequest::new("W");
    first.spans[0].font_size = 64.0;
    let first = system.shape(&first).place([0.0; 2]);
    let mut second = TextLayoutRequest::new("A");
    second.spans[0].font_size = 40.0;
    let second = system.shape(&second).place([0.0; 2]);
    let mut overfull = None;
    atlas.begin_frame();
    let placed = prepare(&mut system, &mut atlas, &first, TextRenderMode::Raster);
    assert!(!placed.instances.is_empty());
    let blocked = system.prepare_glyphs(
        &mut atlas,
        &second,
        TextRenderMode::Raster,
        &[],
        &mut overfull,
    );
    assert!(blocked.dropped_glyphs > 0);
    atlas.commit(atlas.frame);
    atlas.begin_frame();
    let suppressed = system.prepare_glyphs(
        &mut atlas,
        &second,
        TextRenderMode::Raster,
        &[],
        &mut overfull,
    );
    assert!(suppressed.uploads.is_empty());
    atlas.begin_frame();
    let recovered = system.prepare_glyphs(
        &mut atlas,
        &second,
        TextRenderMode::Raster,
        &[],
        &mut overfull,
    );
    assert_eq!(recovered.dropped_glyphs, 0);
    assert!(!recovered.uploads.is_empty());
}

#[test]
fn replacing_one_glyph_in_full_atlas_evicts_only_stale_glyph() {
    let mut system = TextSystem::new();
    let mut request = TextLayoutRequest::new("ABCDE");
    request.spans[0].font_size = 24.0;
    let mut layout = system.shape(&request).place([0.0; 2]);
    let mut atlas = GlyphAtlas::new(64);
    atlas.begin_frame();
    let mut origins = Vec::new();
    for glyph in &layout.glyphs[..4] {
        let (slot, origin) = atlas.allocate([32, 32]).unwrap();
        let placed = PlacedGlyph {
            slot,
            origin,
            size: [32, 32],
            left: 0.0,
            top: 0.0,
            mode: GLYPH_MODE_RASTER_MASK,
        };
        atlas.insert(AtlasKey::Raster(glyph.key.0), AtlasImage::Placed(placed));
        origins.push(origin);
    }
    atlas.commit(atlas.frame);
    assert!(atlas.allocate([1, 1]).is_none());
    let replaced_key = AtlasKey::Raster(layout.glyphs.remove(3).key.0);

    atlas.begin_frame();
    let replaced = prepare(&mut system, &mut atlas, &layout, TextRenderMode::Raster);
    assert_eq!(replaced.dropped_glyphs, 0);
    assert!(!atlas.glyphs.contains_key(&replaced_key));
    for (glyph, origin) in layout.glyphs.iter().zip(origins).take(3) {
        let AtlasImage::Placed(placed) = atlas.glyphs[&AtlasKey::Raster(glyph.key.0)].image else {
            panic!("glyph should stay placed");
        };
        assert_eq!(placed.origin, origin);
    }
    assert!(!replaced.uploads.is_empty());
}

#[test]
fn font_size_sweep_keeps_raster_memory_bounded() {
    let mut system = TextSystem::new();
    let mut atlas = GlyphAtlas::new(2048);
    let mut seen = HashSet::new();
    for frame in 0..60 {
        let mut request = TextLayoutRequest::new("Lumen text");
        request.spans[0].font_size = 32.0 + frame as f32 * (568.0 / 59.0);
        let layout = system.shape(&request).place([0.0; 2]);
        seen.extend(layout.glyphs.iter().map(|glyph| glyph.key));
        atlas.begin_frame();
        let prepared = prepare(&mut system, &mut atlas, &layout, TextRenderMode::Raster);
        atlas.commit(atlas.frame);
        assert_eq!(prepared.dropped_glyphs, 0, "frame {frame}");
    }

    assert!(atlas.glyphs.len() < seen.len() / 2);
}

#[test]
fn uncommitted_uploads_are_repeated() {
    let mut system = TextSystem::new();
    let mut atlas = GlyphAtlas::new(256);
    let layout = system
        .shape(&TextLayoutRequest::new("Retry"))
        .place([0.0; 2]);

    atlas.begin_frame();
    let dropped = prepare(&mut system, &mut atlas, &layout, TextRenderMode::Raster);
    atlas.begin_frame();
    let retried = prepare(&mut system, &mut atlas, &layout, TextRenderMode::Raster);
    atlas.commit(atlas.frame);
    atlas.begin_frame();
    let committed = prepare(&mut system, &mut atlas, &layout, TextRenderMode::Raster);

    assert!(!dropped.uploads.is_empty());
    assert_eq!(retried.uploads.len(), dropped.uploads.len());
    assert_eq!(retried.uploads[0].origin, dropped.uploads[0].origin);
    assert!(committed.uploads.is_empty());
}

#[test]
fn whitespace_draws_no_instances_at_any_subpixel_bin() {
    let mut system = TextSystem::new();
    let mut request = TextLayoutRequest::new(format!("{}ab", " ".repeat(200)));
    request.spans[0].font_size = 8.0;
    let layout = system.shape(&request).place([0.0; 2]);
    let mut atlas = GlyphAtlas::new(1024);
    atlas.begin_frame();
    let prepared = prepare(&mut system, &mut atlas, &layout, TextRenderMode::Raster);
    assert_eq!(prepared.instances.len(), 2);
}

#[test]
fn layouts_share_the_atlas_within_a_frame() {
    let mut system = TextSystem::new();
    let mut atlas = GlyphAtlas::new(1024);
    let unique: String = ('!'..='~')
        .chain('¡'..='ſ')
        .chain('Α'..='Ω')
        .chain('α'..='ω')
        .chain('А'..='я')
        .collect();
    assert!(unique.chars().count() > 400);
    let split = unique
        .char_indices()
        .nth(unique.chars().count() / 2)
        .unwrap()
        .0;
    let (first, second) = unique.split_at(split);
    let layout = |system: &mut TextSystem, content: &str| {
        let mut request = TextLayoutRequest::new(content);
        request.spans[0].font_size = 28.0;
        request.max_width = Some(900.0);
        system.shape(&request).place([0.0; 2])
    };
    let first = layout(&mut system, first);
    let second = layout(&mut system, second);

    for mode in [TextRenderMode::Raster, TextRenderMode::Msdf] {
        let frame = atlas.begin_frame();
        let a = prepare(&mut system, &mut atlas, &first, mode);
        let b = prepare(&mut system, &mut atlas, &second, mode);
        atlas.commit(frame);

        assert_eq!((a.dropped_glyphs, b.dropped_glyphs), (0, 0), "{mode:?}");
        let first_glyphs = atlas.keys_for(&first.glyphs, mode, &[]);
        assert!(
            first_glyphs
                .iter()
                .all(|key| atlas.glyphs.contains_key(key))
        );
    }
}

mod msdf_tests {
    use super::*;
    use crate::msdf;

    const COLR_FONT: &[u8] = include_bytes!("../assets/roboto/RobotoColrTest.ttf");

    fn msdf_instances(prepared: &PreparedGlyphs) -> usize {
        prepared
            .instances
            .iter()
            .filter(|instance| instance.render_mode() == GLYPH_MODE_MSDF)
            .count()
    }

    #[test]
    fn small_raster_fallbacks_share_the_msdf_baseline() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("SMALL SIZES AT FRACTIONAL POSITIONS");
        request.spans[0].font_size = 14.0;
        let shaped = system.shape(&request);
        let mut atlas = GlyphAtlas::new(1024);
        for y in [24.0, 24.6, -24.6] {
            let layout = shaped.place([40.3, y]);
            atlas.begin_frame();
            // Exercise mixed rendering without depending on system fonts' overlaps.
            atlas.insert(
                AtlasKey::Msdf(msdf::msdf_key(layout.glyphs[0].key.0)),
                AtlasImage::RasterOnly,
            );
            let prepared = prepare(&mut system, &mut atlas, &layout, TextRenderMode::Msdf);
            assert!(msdf_instances(&prepared) > 0);
            assert!(msdf_instances(&prepared) < prepared.instances.len());
            let visible = layout.glyphs.iter().filter_map(|glyph| {
                let key = atlas.key_for(glyph.key, TextRenderMode::Msdf);
                match atlas.glyphs[&key].image {
                    AtlasImage::Placed(placed) => Some((glyph, placed)),
                    _ => None,
                }
            });
            for ((glyph, placed), instance) in visible.zip(&prepared.instances) {
                let baseline = if instance.render_mode() == GLYPH_MODE_MSDF {
                    instance.rect[1]
                        + (placed.top - 0.5) * request.spans[0].font_size
                            / msdf_em_size(request.spans[0].font_size)
                } else {
                    // Swash's raster offset is in outline coordinates, where Y points up.
                    instance.rect[1] + placed.top - glyph.key.0.y_bin.as_float()
                };
                assert!(
                    (baseline - glyph.position[1]).abs() < 0.125,
                    "glyph {:?}: rendered baseline {baseline}, layout baseline {}",
                    glyph.key,
                    glyph.position[1],
                );
            }
        }
    }

    #[test]
    fn font_size_sweep_generates_each_field_once() {
        let mut system = TextSystem::new();
        let mut atlas = GlyphAtlas::new(4096);
        let mut uploads = 0;
        let mut fields = HashSet::new();
        for frame in 0..40 {
            let mut request = TextLayoutRequest::new("Lumen MSDF text");
            request.spans[0].font_size = 32.0 + frame as f32 * (968.0 / 39.0);
            let layout = system.shape(&request).place([frame as f32 * 0.37, 0.0]);
            fields.extend(
                layout
                    .glyphs
                    .iter()
                    .map(|glyph| msdf::msdf_key(glyph.key.0)),
            );
            atlas.begin_frame();
            let prepared = prepare(&mut system, &mut atlas, &layout, TextRenderMode::Msdf);
            atlas.commit(atlas.frame);

            assert_eq!(prepared.dropped_glyphs, 0, "frame {frame}");
            assert!(msdf_instances(&prepared) > 0);
            uploads += prepared.uploads.iter().filter(|upload| {
                atlas.glyphs.iter().any(|(key, glyph)| {
                    matches!(key, AtlasKey::Msdf(_)) && matches!(glyph.image, AtlasImage::Placed(placed) if placed.origin == upload.origin)
                })
            }).count();
        }

        assert!(uploads <= fields.len());
    }

    #[test]
    fn field_resolution_grows_with_font_size() {
        assert_eq!(msdf_em_size(14.0), MSDF_EM_SIZE);
        assert_eq!(msdf_em_size(4.0 * MSDF_EM_SIZE), MSDF_EM_SIZE);
        assert_eq!(msdf_em_size(4.0 * MSDF_EM_SIZE + 1.0), 2.0 * MSDF_EM_SIZE);
        assert_eq!(msdf_em_size(1000.0), MSDF_MAX_EM_SIZE);
        assert_eq!(msdf_em_size(100_000.0), MSDF_MAX_EM_SIZE);
    }

    #[test]
    fn oversized_raster_glyphs_fall_back_to_distance_fields() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("H");
        request.spans[0].font_size = 1500.0;
        let layout = system.shape(&request).place([0.0; 2]);
        let mut atlas = GlyphAtlas::new(512);
        atlas.begin_frame();
        let raster = prepare(&mut system, &mut atlas, &layout, TextRenderMode::Raster);
        assert_eq!(raster.dropped_glyphs, 0);
        assert_eq!(msdf_instances(&raster), 1);
        atlas.commit(atlas.frame);

        // The oversized raster key is remembered, so the next frame does not
        // rasterize it again.
        atlas.begin_frame();
        let again = prepare(&mut system, &mut atlas, &layout, TextRenderMode::Raster);
        assert_eq!(msdf_instances(&again), 1);
        assert!(again.uploads.is_empty());

        // Colour glyphs have no outline to scale, so they are still dropped.
        system.load_font_data(COLR_FONT.to_vec());
        let mut colour = TextLayoutRequest::new("A");
        colour.spans[0].font_family = "Roboto COLR Test".to_string();
        colour.spans[0].font_size = 1500.0;
        let colour = system.shape(&colour).place([0.0; 2]);
        atlas.begin_frame();
        let colour = prepare(&mut system, &mut atlas, &colour, TextRenderMode::Raster);
        assert_eq!(colour.instances.len(), 0);
        assert_eq!(colour.dropped_glyphs, 1);
    }

    #[test]
    fn colour_glyphs_are_not_stroked() {
        let mut system = TextSystem::new();
        system.load_font_data(COLR_FONT.to_vec());
        let mut colour = TextLayoutRequest::new("AA");
        colour.spans[0].font_family = "Roboto COLR Test".to_string();
        let colour = system.shape(&colour).place([0.0; 2]);
        let mut atlas = GlyphAtlas::new(1024);
        atlas.begin_frame();
        let colour = system.prepare_glyphs(
            &mut atlas,
            &colour,
            TextRenderMode::Raster,
            &[1.0],
            &mut None,
        );
        assert_eq!(colour.instances.len(), 2);
        assert_eq!((colour.strokes.len(), colour.dropped_glyphs), (0, 0));
    }

    #[test]
    fn wide_strokes_widen_the_field_range_until_it_outgrows_the_atlas() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("I");
        request.spans[0].font_size = 192.0;
        let layout = system.shape(&request).place([0.0; 2]);
        let mut atlas = GlyphAtlas::new(256);
        for mode in [TextRenderMode::Msdf, TextRenderMode::Raster] {
            atlas.begin_frame();
            let prepared = system.prepare_glyphs(&mut atlas, &layout, mode, &[64.0], &mut None);
            atlas.commit(atlas.frame);
            let [stroke] = prepared.strokes[..] else {
                panic!("{mode:?}: expected one stroke");
            };
            assert_eq!(stroke.render_mode(), GLYPH_MODE_MSDF, "{mode:?}");
            // 48 texels per em, where the stroke reaches 16 texels.
            let tier = (stroke.flags >> GLYPH_RANGE_SHIFT) & GLYPH_RANGE_MASK;
            assert!(
                MSDF_PX_RANGE * (1 << tier) as f32 - MSDF_PX_RANGE / 2.0 - 1.5 >= 16.0,
                "{mode:?}"
            );
        }
    }

    #[test]
    fn fields_too_large_for_the_atlas_are_dropped_before_they_are_generated() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("I");
        request.spans[0].font_size = 48.0;
        let layout = system.shape(&request).place([0.0; 2]);
        let mut atlas = GlyphAtlas::new(128);
        let mode = TextRenderMode::Msdf;
        for half_width in [1000.0, 7000.0] {
            let start = std::time::Instant::now();
            atlas.begin_frame();
            let prepared =
                system.prepare_glyphs(&mut atlas, &layout, mode, &[half_width], &mut None);
            assert_eq!(prepared.instances.len(), 1, "{half_width}");
            assert_eq!(
                (prepared.strokes.len(), prepared.dropped_glyphs),
                (0, 1),
                "{half_width}"
            );
            // Only the glyph's own field is uploaded, never the stroke's.
            assert_eq!(prepared.uploads.len(), 1, "{half_width}");
            // Generating the stroke's field and then discarding it would take far longer.
            assert!(
                start.elapsed() < std::time::Duration::from_secs(1),
                "{half_width}"
            );
        }
    }

    #[test]
    fn unused_field_markers_are_forgotten() {
        let mut system = TextSystem::new();
        let mut atlas = GlyphAtlas::new(256);
        let mut prepare_text = |atlas: &mut GlyphAtlas, text: &str| {
            let layout = system.shape(&TextLayoutRequest::new(text)).place([0.0; 2]);
            atlas.begin_frame();
            prepare(&mut system, atlas, &layout, TextRenderMode::Msdf);
        };
        prepare_text(&mut atlas, "A B");
        assert!(atlas.glyphs.len() > 2);
        prepare_text(&mut atlas, "A");
        prepare_text(&mut atlas, "A");
        assert!(
            atlas
                .glyphs
                .values()
                .all(|glyph| matches!(glyph.image, AtlasImage::Placed(_)))
        );
    }

    #[test]
    fn animated_widths_keep_an_overfull_field_working_set() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("IOH");
        request.spans[0].font_size = 48.0;
        let layout = system.shape(&request).place([0.0; 2]);
        let mut atlas = GlyphAtlas::new(64);
        let mut overfull = None;
        for (frame, half_width) in [0.25, 0.25, 0.4, 0.7, 0.9].into_iter().enumerate() {
            atlas.begin_frame();
            let prepared = system.prepare_glyphs(
                &mut atlas,
                &layout,
                TextRenderMode::Msdf,
                &[half_width],
                &mut overfull,
            );
            atlas.commit(atlas.frame);
            assert!(prepared.dropped_glyphs > 0, "frame {frame}");
            if frame > 0 {
                assert!(prepared.uploads.is_empty(), "frame {frame} uploaded");
            }
        }
    }

    #[test]
    fn moving_the_origin_keeps_an_overfull_field_working_set() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("IOH");
        request.spans[0].font_size = 48.0;
        let shaped = system.shape(&request);
        let mut atlas = GlyphAtlas::new(64);
        let mut overfull = None;
        for (frame, origin) in [0.0, 0.0, 0.5].into_iter().enumerate() {
            let layout = shaped.place([origin, 0.0]);
            atlas.begin_frame();
            let prepared = system.prepare_glyphs(
                &mut atlas,
                &layout,
                TextRenderMode::Msdf,
                &[0.25],
                &mut overfull,
            );
            atlas.commit(atlas.frame);
            assert!(prepared.dropped_glyphs > 0, "frame {frame}");
            // Fields do not depend on the subpixel position, so the working set is the same.
            if frame > 0 {
                assert!(prepared.uploads.is_empty(), "frame {frame} uploaded");
            }
        }
    }

    #[test]
    fn oversized_fallbacks_stay_stable_when_the_atlas_overflows() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("HWI");
        request.spans[0].font_size = 1500.0;
        let layout = system.shape(&request).place([0.0; 2]);
        let mut atlas = GlyphAtlas::new(512);
        let mut overfull = None;
        let mut frames = Vec::new();
        for _ in 0..6 {
            atlas.begin_frame();
            let prepared = system.prepare_glyphs(
                &mut atlas,
                &layout,
                TextRenderMode::Raster,
                &[],
                &mut overfull,
            );
            atlas.commit(atlas.frame);
            frames.push((prepared.instances.len(), prepared.uploads.len()));
        }
        let (instances, _) = frames[0];
        assert!(instances > 0);
        for (frame, (count, uploads)) in frames.iter().enumerate() {
            assert_eq!(*count, instances, "frame {frame}");
            assert!(frame == 0 || *uploads == 0, "frame {frame} uploaded");
        }
    }

    #[test]
    fn auto_mode_uses_fields_from_the_size_threshold() {
        let mut system = TextSystem::new();
        let mut atlas = GlyphAtlas::new(2048);
        for (size, fields) in [
            (MSDF_AUTO_MIN_SIZE - 1.0, 0),
            (MSDF_AUTO_MIN_SIZE, 1),
            (MSDF_AUTO_MIN_SIZE + 100.0, 1),
        ] {
            let mut request = TextLayoutRequest::new("H");
            request.spans[0].font_size = size;
            let layout = system.shape(&request).place([0.0; 2]);
            atlas.begin_frame();
            let prepared = prepare(&mut system, &mut atlas, &layout, TextRenderMode::Auto);
            assert_eq!(prepared.dropped_glyphs, 0);
            assert_eq!(msdf_instances(&prepared), fields, "size {size}");
            assert_eq!(prepared.instances.len(), 1);
        }
    }

    /// Pixels of `window` (`[x, y, width, height]`) covered by one glyph, as the
    /// text shader computes them: raster glyphs read 1:1, fields are
    /// bilinearly filtered and sharpened.
    fn glyph_coverage(
        system: &mut TextSystem,
        layout: &TextLayout,
        mode: TextRenderMode,
        window: [i32; 4],
    ) -> Vec<f32> {
        const ATLAS: u32 = 4096;
        let render = system.render_atlas(layout, mode, ATLAS);
        let [instance] = render.instances[..] else {
            panic!("expected one glyph");
        };
        let [x, y, width, height] = instance.rect;
        let [origin_x, origin_y, window_width, window_height] = window;
        let texel = |x: i32, y: i32| -> [f32; 4] {
            let at = (y as usize * ATLAS as usize + x as usize) * 4;
            std::array::from_fn(|i| f32::from(render.pixels[at + i]) / 255.0)
        };
        let [u0, v0, u1, _] = instance.uv_rect;
        let texels_per_pixel = (u1 - u0) / width;
        let screen_px_range = (0.5 * MSDF_PX_RANGE / texels_per_pixel).max(1.0);
        let mut coverage = Vec::new();
        for py in 0..window_height {
            for px in 0..window_width {
                let (fx, fy) = (
                    (origin_x + px) as f32 + 0.5 - x,
                    (origin_y + py) as f32 + 0.5 - y,
                );
                let (inside_x, inside_y) =
                    ((0.0..width).contains(&fx), (0.0..height).contains(&fy));
                coverage.push(if !(inside_x && inside_y) {
                    0.0
                } else if instance.render_mode() == GLYPH_MODE_MSDF {
                    let (u, v) = (
                        u0 + fx * texels_per_pixel - 0.5,
                        v0 + fy * texels_per_pixel - 0.5,
                    );
                    let (tx, ty) = (u.floor(), v.floor());
                    let (wx, wy) = (u - tx, v - ty);
                    let mut median = [0.0; 3];
                    for (channel, value) in median.iter_mut().enumerate() {
                        let at = |dx: i32, dy: i32| texel(tx as i32 + dx, ty as i32 + dy)[channel];
                        *value = (at(0, 0) * (1.0 - wx) + at(1, 0) * wx) * (1.0 - wy)
                            + (at(0, 1) * (1.0 - wx) + at(1, 1) * wx) * wy;
                    }
                    let [r, g, b] = median;
                    let median = r.min(g).max(r.max(g).min(b));
                    (screen_px_range * (median - 0.5) + 0.5).clamp(0.0, 1.0)
                } else {
                    texel((u0 + fx) as i32, (v0 + fy) as i32)[3]
                });
            }
        }
        coverage
    }

    /// Fields drawn at 1000 px must stay within a pixel of the raster outline.
    /// A field with 48 texels per em cuts facets of more than a pixel into the
    /// sharp corners and curves of these glyphs.
    #[test]
    fn large_fields_keep_corners_sharp() {
        let mut system = TextSystem::new();
        for text in ["R", "W", "a", "4"] {
            let mut request = TextLayoutRequest::new(text);
            request.spans[0].font_size = 1000.0;
            let layout = system.shape(&request).place([0.0; 2]);
            let [x, y] = layout.glyphs[0].position.map(|axis| axis.floor() as i32);
            let window = [x - 50, y - 800, 800, 1100];
            let raster = glyph_coverage(&mut system, &layout, TextRenderMode::Raster, window);
            let msdf = glyph_coverage(&mut system, &layout, TextRenderMode::Msdf, window);
            let (width, height) = (window[2] as usize, window[3] as usize);
            let mut outliers = 0;
            for py in 1..height - 1 {
                for px in 1..width - 1 {
                    let (mut low, mut high) = (1.0_f32, 0.0_f32);
                    for ny in py - 1..=py + 1 {
                        for nx in px - 1..=px + 1 {
                            low = low.min(raster[ny * width + nx]);
                            high = high.max(raster[ny * width + nx]);
                        }
                    }
                    let value = msdf[py * width + px];
                    outliers += usize::from((low - value).max(value - high) > 0.1);
                }
            }
            assert!(outliers <= 16, "{text}: {outliers} pixels off the outline");
        }
    }

    #[test]
    fn colour_and_overlapping_glyphs_fall_back_to_raster() {
        let mut system = TextSystem::new();
        system.load_font_data(COLR_FONT.to_vec());
        let mut colour = TextLayoutRequest::new("A");
        colour.spans[0].font_family = "Roboto COLR Test".to_string();
        colour.spans[0].font_size = 48.0;
        let colour = system.shape(&colour).place([0.0; 2]);

        let mut overlapping = system.shape(&TextLayoutRequest::new("O")).place([0.0; 2]);
        let mut atlas = GlyphAtlas::new(512);
        atlas.begin_frame();
        let colour = prepare(&mut system, &mut atlas, &colour, TextRenderMode::Msdf);
        assert_eq!(colour.instances.len(), 1);
        assert_eq!(colour.instances[0].render_mode(), GLYPH_MODE_RASTER_COLOR);

        // Two copies of one outline overlap everywhere.
        let key = msdf::msdf_key(overlapping.glyphs[0].key.0);
        let commands: Vec<_> = system
            .rasterizer
            .scale_outline(&mut system.font_system, key)
            .unwrap()
            .commands()
            .collect();
        let overlaps = |commands: &[zeno::Command]| {
            msdf::contours_overlap(&msdf::contours(commands.iter().copied(), |p| {
                fdsm::bezier::Point::new(f64::from(p.x), f64::from(p.y))
            }))
        };
        assert!(!overlaps(&commands));
        assert!(overlaps(&[&commands[..], &commands[..]].concat()));

        overlapping.glyphs[0].key.0.flags |= CacheKeyFlags::FAKE_ITALIC;
        let italic = prepare(&mut system, &mut atlas, &overlapping, TextRenderMode::Msdf);
        assert_eq!(msdf_instances(&italic), 1);
    }
}
