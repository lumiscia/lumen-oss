use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use cosmic_text::{CacheKey, SwashContent, SwashImage};
use etagere::{AllocId, size2};
use swash::scale::Source;

use crate::gpu::{
    GLYPH_MODE_MSDF, GLYPH_MODE_RASTER_COLOR, GLYPH_MODE_RASTER_MASK, GLYPH_RANGE_SHIFT,
    GpuGlyphInstance,
};
use crate::msdf::{self, MSDF_AUTO_MIN_SIZE};
use crate::shape::{GlyphKey, ShapedText, TextGlyph, TextLayout};
use crate::{MAX_GLYPH_EXTENTS, TextSystem};

/// How outline glyphs are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TextRenderMode {
    /// Glyphs are rasterized for their size and subpixel position.
    #[default]
    Raster,
    /// Outline glyphs share distance fields across sizes, regenerated at a
    /// finer resolution for each doubling of size past [`MSDF_EM_SIZE`] × 6.
    /// Colour, bitmap and overlapping-contour glyphs are rasterized.
    Msdf,
    /// [`Self::Raster`] below [`MSDF_AUTO_MIN_SIZE`] and [`Self::Msdf`] from it.
    Auto,
}

impl TextSystem {
    /// The pixels `shaped` may draw to, as `[min_x, min_y, max_x, max_y]` from its origin, in any
    /// render mode. This is the union of its glyph outlines grown by a margin for antialiasing,
    /// subpixel snapping and distance field padding. `None` when the bounds are not finite.
    pub fn ink_bounds(&mut self, shaped: &ShapedText) -> Option<[f32; 4]> {
        if self.glyph_extents.len() > MAX_GLYPH_EXTENTS {
            self.glyph_extents.clear();
        }
        let mut ink: Option<[f32; 4]> = None;
        for shaped_glyph in &shaped.glyphs {
            let glyph = &shaped_glyph.glyph;
            let position = [
                glyph.x + glyph.font_size * glyph.x_offset,
                glyph.y - glyph.font_size * glyph.y_offset + shaped_glyph.line_y,
            ];
            let (key, ..) = CacheKey::new(
                glyph.font_id,
                glyph.glyph_id,
                glyph.font_size,
                (0.0, 0.0),
                glyph.font_weight,
                glyph.cache_key_flags,
            );
            let outline = match self.glyph_extents.get(&key) {
                Some(outline) => *outline,
                None => {
                    let outline = self.glyph_extent(key);
                    self.glyph_extents.insert(key, outline);
                    outline
                }
            };
            let Some([min_x, min_y, max_x, max_y]) = outline else {
                continue;
            };
            let margin = 2.0 + 0.1 * glyph.font_size;
            let glyph_ink = [
                position[0] + min_x - margin,
                position[1] - max_y - margin,
                position[0] + max_x + margin,
                position[1] - min_y + margin,
            ];
            ink = Some(match ink {
                Some(ink) => [
                    ink[0].min(glyph_ink[0]),
                    ink[1].min(glyph_ink[1]),
                    ink[2].max(glyph_ink[2]),
                    ink[3].max(glyph_ink[3]),
                ],
                None => glyph_ink,
            });
        }
        let bounds = ink.unwrap_or([0.0; 4]);
        bounds
            .iter()
            .all(|value| value.is_finite())
            .then_some(bounds)
    }

    /// A glyph's `[min_x, min_y, max_x, max_y]` with Y pointing up from its pen position, or
    /// `None` when it draws nothing.
    fn glyph_extent(&mut self, key: CacheKey) -> Option<[f32; 4]> {
        if self.is_plain_outline(key)
            && let Some(commands) = self
                .swash_cache
                .get_outline_commands_uncached(&mut self.font_system, key)
        {
            return msdf::outline_bounds(&commands);
        }
        let placement = self
            .swash_cache
            .get_image_uncached(&mut self.font_system, key)?
            .placement;
        (placement.width > 0 && placement.height > 0).then_some([
            placement.left as f32,
            placement.top as f32 - placement.height as f32,
            placement.left as f32 + placement.width as f32,
            placement.top as f32,
        ])
    }

    /// Whether the glyph's font draws it from its outline alone, with no colour or bitmap tables.
    fn is_plain_outline(&mut self, key: CacheKey) -> bool {
        self.font_system
            .get_font(key.font_id, key.font_weight)
            .is_some_and(|font| !msdf::has_colour_tables(&font))
    }

    /// Whether `key` is an outline glyph with nothing to draw, such as whitespace.
    fn is_blank_outline(&mut self, key: CacheKey) -> bool {
        self.is_plain_outline(key)
            && self
                .swash_cache
                .get_outline_commands_uncached(&mut self.font_system, key)
                .is_none_or(|commands| commands.is_empty())
    }

    /// Places `layout` in `atlas` for its current frame, generating glyphs that
    /// are not resident yet one at a time.
    ///
    /// When the atlas is full, the least recently used glyphs that no layout
    /// used this frame are evicted. If there is still no room and only this
    /// layout uses the atlas this frame, the atlas is cleared and repacked.
    /// Glyphs that still do not fit are dropped.
    /// `overfull` suppresses retries until the working set or reclaimable space
    /// changes. Uploads become resident after [`GlyphAtlas::commit`].
    ///
    /// The outline glyphs of each span `strokes` gives a positive half width are stroked by that
    /// many pixels to each side. A stroke follows the path its glyph takes: a true distance field
    /// where the glyph draws one, else the stroked raster outline, or a true distance field where
    /// that is too large for the atlas. A field's range doubles as often as the stroke needs, and
    /// a stroke whose field is too large for the atlas is dropped.
    pub fn prepare_glyphs(
        &mut self,
        atlas: &mut GlyphAtlas,
        layout: &TextLayout,
        mode: TextRenderMode,
        strokes: &[f32],
        overfull: &mut Option<u64>,
    ) -> PreparedGlyphs {
        if atlas
            .font_generation
            .replace(self.font_generation)
            .is_some_and(|prior| prior != self.font_generation)
        {
            atlas.clear();
        }
        let glyphs = &layout.glyphs;
        let overflow_key = |atlas: &mut GlyphAtlas| {
            let mut keys = atlas.keys_for(glyphs, mode, strokes);
            keys.sort_unstable();
            let mut hasher = DefaultHasher::new();
            (keys, atlas.availability).hash(&mut hasher);
            hasher.finish()
        };
        let retry_new = overfull.is_none_or(|key| key != overflow_key(atlas));
        let mut uploads = Vec::new();
        if !self.place_glyphs(atlas, glyphs, mode, strokes, retry_new, &mut uploads)
            && atlas.layouts_this_frame == 0
        {
            atlas.clear();
            uploads.clear();
            self.place_glyphs(atlas, glyphs, mode, strokes, true, &mut uploads);
        }

        let mut instances = Vec::with_capacity(glyphs.len());
        let mut sources = Vec::with_capacity(glyphs.len());
        let mut stroke_instances = Vec::new();
        let mut stroke_glyphs = Vec::new();
        let mut capacity_dropped = 0;
        for (index, glyph) in glyphs.iter().enumerate() {
            let key = atlas.key_for(glyph.key, mode);
            match atlas.glyphs.get(&key) {
                Some(AtlasGlyph {
                    image: AtlasImage::Placed(placed),
                    ..
                }) => {
                    instances.push(placed.instance(glyph, key));
                    sources.push(index);
                    let Some(half_width) = glyph_stroke(strokes, glyph) else {
                        continue;
                    };
                    let key = atlas.stroke_key_for(glyph.key, mode, half_width);
                    match atlas.glyphs.get(&key).map(|stroke| stroke.image) {
                        Some(AtlasImage::Placed(stroke)) => {
                            stroke_instances.push(stroke.instance(glyph, key));
                            stroke_glyphs.push(instances.len() - 1);
                        }
                        Some(AtlasImage::Empty) => {}
                        _ => capacity_dropped += 1,
                    }
                }
                Some(AtlasGlyph {
                    image: AtlasImage::Empty,
                    ..
                }) => {}
                None if self.is_blank_outline(glyph.key.0) => {
                    atlas.insert(key, AtlasImage::Empty);
                }
                _ => {
                    // The stroke is omitted along with its glyph.
                    let stroked = glyph_stroke(strokes, glyph).is_some()
                        && self.is_plain_outline(glyph.key.0);
                    capacity_dropped += 1 + usize::from(stroked);
                }
            }
        }
        *overfull = (capacity_dropped > 0).then(|| overflow_key(atlas));

        atlas.layouts_this_frame += 1;
        PreparedGlyphs {
            instances,
            sources,
            strokes: stroke_instances,
            stroke_glyphs,
            uploads,
            dropped_glyphs: capacity_dropped,
        }
    }

    /// Returns `false` when the atlas ran out of room.
    fn place_glyphs(
        &mut self,
        atlas: &mut GlyphAtlas,
        glyphs: &[TextGlyph],
        mode: TextRenderMode,
        strokes: &[f32],
        retry_new: bool,
        uploads: &mut Vec<GlyphUpload>,
    ) -> bool {
        // A second pass places raster glyphs for distance fields that turned
        // out to be raster-only.
        loop {
            let keys = atlas.keys_for(glyphs, mode, strokes);
            atlas.touch(&keys);
            let markers = atlas.markers;
            if !self.place_missing(atlas, &keys, retry_new, uploads) {
                return false;
            }
            if atlas.markers == markers || atlas.keys_for(glyphs, mode, strokes) == keys {
                return true;
            }
        }
    }

    /// Renders and places the glyphs of `keys` that are not resident, writing
    /// each one before rendering the next. Returns `false` when the atlas has no
    /// room for the next glyph. Glyphs not already cached are skipped unless
    /// `retry_new`.
    fn place_missing(
        &mut self,
        atlas: &mut GlyphAtlas,
        keys: &[AtlasKey],
        retry_new: bool,
        uploads: &mut Vec<GlyphUpload>,
    ) -> bool {
        for &key in keys {
            let cached = atlas.glyphs.get(&key);
            if cached.is_some_and(|glyph| glyph.resident || glyph.upload_frame == atlas.frame)
                || (cached.is_none() && !retry_new)
            {
                continue;
            }
            let cached = cached.and_then(|glyph| match glyph.image {
                AtlasImage::Placed(placed) => Some(placed),
                _ => None,
            });
            let pixels = match self.render_glyph(key, atlas.size) {
                RenderedGlyph::Pixels(pixels) => pixels,
                RenderedGlyph::Empty => {
                    atlas.insert(key, AtlasImage::Empty);
                    continue;
                }
                RenderedGlyph::Oversized => {
                    atlas.insert(key, AtlasImage::Oversized);
                    continue;
                }
                RenderedGlyph::RasterOnly => {
                    atlas.insert(key, AtlasImage::RasterOnly);
                    continue;
                }
            };
            if pixels.size.iter().any(|side| *side > atlas.size) {
                atlas.insert(key, AtlasImage::Oversized);
                continue;
            }
            let placed = match cached {
                Some(placed) => placed,
                None => {
                    let Some((slot, origin)) = atlas.allocate(pixels.size) else {
                        return false;
                    };
                    PlacedGlyph {
                        slot,
                        origin,
                        size: pixels.size,
                        left: pixels.left,
                        top: pixels.top,
                        mode: pixels.mode,
                    }
                }
            };
            uploads.push(GlyphUpload {
                origin: placed.origin,
                size: placed.size,
                pixels: pixels.pixels,
            });
            atlas.insert(key, AtlasImage::Placed(placed));
        }
        true
    }

    fn render_glyph(&mut self, key: AtlasKey, max_size: u32) -> RenderedGlyph {
        match key {
            AtlasKey::Raster(key) => {
                if self.is_plain_outline(key)
                    && let Some(commands) = self
                        .swash_cache
                        .get_outline_commands_uncached(&mut self.font_system, key)
                    && let Some([min_x, min_y, max_x, max_y]) = msdf::outline_bounds(&commands)
                {
                    let dx = key.x_bin.as_float();
                    let dy = key.y_bin.as_float();
                    let width = (max_x + dx).ceil() - (min_x + dx).floor();
                    let height = (max_y + dy).ceil() - (min_y + dy).floor();
                    if width > max_size as f32 || height > max_size as f32 {
                        return RenderedGlyph::Oversized;
                    }
                }
                self.swash_cache
                    .get_image_uncached(&mut self.font_system, key)
                    .filter(|image| {
                        image.placement.width > 0
                            && image.placement.height > 0
                            && image.data.iter().any(|&byte| byte != 0)
                    })
                    .map_or(RenderedGlyph::Empty, |image| {
                        RenderedGlyph::Pixels(raster_pixels(&image, key))
                    })
            }
            AtlasKey::RasterStroke(key, half_width) => {
                self.render_stroke(key, f32::from_bits(half_width), max_size)
            }
            AtlasKey::Msdf(key) => msdf::render_msdf_glyph(
                &mut self.font_system,
                &mut self.swash_cache,
                key,
                None,
                max_size,
            ),
            AtlasKey::Sdf(key, tier) => msdf::render_msdf_glyph(
                &mut self.font_system,
                &mut self.swash_cache,
                key,
                Some(tier),
                max_size,
            ),
        }
    }

    /// A plain outline stroked `half_width` pixels to each side with round joins, placed as
    /// Swash places its fill.
    fn render_stroke(&mut self, key: CacheKey, half_width: f32, max_size: u32) -> RenderedGlyph {
        use cosmic_text::CacheKeyFlags;
        use swash::zeno::{Angle, Join, Mask, Origin, Stroke, Transform, Vector};

        if half_width == 0.0 || !self.is_plain_outline(key) {
            return RenderedGlyph::Empty;
        }
        let Some(font) = self.font_system.get_font(key.font_id, key.font_weight) else {
            return RenderedGlyph::Empty;
        };
        let swash_font = font.as_swash();
        let mut scaler = self
            .scale_context
            .0
            .builder(swash_font)
            .size(f32::from_bits(key.font_size_bits))
            .hint(!key.flags.contains(CacheKeyFlags::DISABLE_HINTING));
        if let Some(variation) = swash_font
            .variations()
            .find_by_tag(swash::Tag::from_be_bytes(*b"wght"))
        {
            scaler = scaler.normalized_coords(swash_font.variations().normalized_coords([(
                swash::Tag::from_be_bytes(*b"wght"),
                f32::from(key.font_weight.0).clamp(variation.min_value(), variation.max_value()),
            )]));
        }
        let mut scaler = scaler.build();
        let Some(mut outline) = scaler.scale_outline(key.glyph_id) else {
            return RenderedGlyph::Empty;
        };
        if key.flags.contains(CacheKeyFlags::FAKE_ITALIC) {
            outline.transform(&Transform::skew(
                Angle::from_degrees(14.0),
                Angle::from_degrees(0.0),
            ));
        }
        let offset = Vector::new(key.x_bin.as_float(), key.y_bin.as_float());
        let mut stroke = Stroke::new(2.0 * half_width);
        stroke.join(Join::Round);
        let mut mask = Mask::new((outline.points(), outline.verbs()));
        let mut size = [0; 2];
        mask.style(stroke)
            .origin(Origin::BottomLeft)
            .offset(offset)
            .render_offset(offset)
            .inspect(|_, width, height| size = [width, height]);
        if size.iter().any(|side| *side > max_size) {
            return RenderedGlyph::Oversized;
        }
        if size.contains(&0) {
            return RenderedGlyph::Empty;
        }
        let (alpha, placement) = mask.render();
        RenderedGlyph::Pixels(GlyphPixels {
            size: [placement.width, placement.height],
            left: placement.left as f32,
            top: placement.top as f32,
            mode: GLYPH_MODE_RASTER_MASK,
            pixels: alpha.into_iter().flat_map(|alpha| [alpha; 4]).collect(),
        })
    }
}

/// The half width below which a raster stroke's coverage at the outline, twice its half
/// width, rounds to nothing in 8-bit alpha. Distance-field strokes fade out continuously
/// instead, as they may be magnified.
const MIN_STROKE_HALF_WIDTH: f32 = 1.0 / 512.0;

/// The half width of the stroke `strokes` gives `glyph`'s span, if it has one.
fn glyph_stroke(strokes: &[f32], glyph: &TextGlyph) -> Option<f32> {
    strokes
        .get(glyph.item.span as usize)
        .copied()
        .filter(|half_width| *half_width > 0.0)
}

/// How far past the outline a stroke `half_width` pixels to each side may draw: further than
/// `half_width` where its raster stroke rounds it up.
pub fn stroke_reach(half_width: f32) -> f32 {
    half_width.max(f32::from_bits(stroke_width_key(half_width)))
}

/// The bits of `half_width` rounded for a raster stroke's key: to quarter pixels from 0.25 px
/// to 4 px, and otherwise to a sixteenth of its power of two, so animated widths reuse
/// strokes. Below a quarter pixel a stroke's coverage follows its width, which the relative
/// rounding keeps from jumping, down to [`MIN_STROKE_HALF_WIDTH`], below which it is 0.
fn stroke_width_key(half_width: f32) -> u32 {
    if half_width < MIN_STROKE_HALF_WIDTH {
        0
    } else if (0.25..4.0).contains(&half_width) {
        ((half_width * 4.0).round() / 4.0).to_bits()
    } else {
        (half_width.to_bits() + (1 << 18)) & !((1 << 19) - 1)
    }
}

/// etagere indexes its items with `u16` and wraps silently past that. Each
/// allocation uses at most two items plus one per shelf, so this keeps atlases
/// up to 4096 pixels tall well within range.
const MAX_ATLAS_GLYPHS: usize = 16 * 1024;

/// Glyph atlas shared by every text layout drawn with one GPU texture.
///
/// Raster glyphs and distance fields share one RGBA8 texture. Only placement
/// is kept on the CPU; glyph pixels exist solely in the uploads returned for
/// a frame. A fixed size keeps every free rectangle available to large glyphs.
pub struct GlyphAtlas {
    allocator: etagere::AtlasAllocator,
    allocations: usize,
    size: u32,
    availability: u64,
    glyphs: foldhash::HashMap<AtlasKey, AtlasGlyph>,
    frame: u64,
    layouts_this_frame: usize,
    /// Eviction candidates for this frame, most recently used first.
    stale: Option<Vec<AtlasKey>>,
    /// Counts inserted `Oversized` and `RasterOnly` markers, which change [`Self::key_for`].
    markers: u64,
    /// The font generation the glyphs are from, as another's font IDs may name different fonts.
    font_generation: Option<u64>,
}

/// Raster glyphs are keyed by size and subpixel bin; distance fields only by
/// font, glyph, weight and fake italic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum AtlasKey {
    Raster(CacheKey),
    /// A raster glyph's outline stroked to each side by the half width whose bits
    /// [`stroke_width_key`] gives.
    RasterStroke(CacheKey, u32),
    Msdf(CacheKey),
    /// A true distance field, which strokes are drawn from, with the range tier from
    /// `msdf::stroke_key`.
    Sdf(CacheKey, u32),
}

impl AtlasKey {
    /// The glyph key; a distance field's holds the field's texels per em as its font size.
    fn cache_key(self) -> CacheKey {
        match self {
            Self::Raster(key) | Self::RasterStroke(key, _) => key,
            Self::Msdf(key) | Self::Sdf(key, _) => key,
        }
    }
}

#[derive(Debug)]
struct AtlasGlyph {
    image: AtlasImage,
    last_used: u64,
    upload_frame: u64,
    resident: bool,
}

#[derive(Debug, Clone, Copy)]
enum AtlasImage {
    /// Nothing to draw, such as whitespace.
    Empty,
    /// A distance-field key whose glyph is drawn as raster instead.
    RasterOnly,
    Oversized,
    Placed(PlacedGlyph),
}

#[derive(Debug, Clone, Copy)]
struct PlacedGlyph {
    slot: AllocId,
    origin: [u32; 2],
    size: [u32; 2],
    /// Image offset from the pen, y up; in pixels for raster glyphs and field
    /// texels for distance fields.
    left: f32,
    top: f32,
    mode: u32,
}

impl std::fmt::Debug for GlyphAtlas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlyphAtlas")
            .field("size", &self.size)
            .field("glyphs", &self.glyphs)
            .finish_non_exhaustive()
    }
}

impl GlyphAtlas {
    pub fn new(size: u32) -> Self {
        let size = size.clamp(1, 4096);
        Self {
            allocator: etagere::AtlasAllocator::new(size2(size as i32, size as i32)),
            allocations: 0,
            size,
            availability: 0,
            glyphs: foldhash::HashMap::default(),
            frame: 0,
            layouts_this_frame: 0,
            stale: None,
            markers: 0,
            font_generation: None,
        }
    }

    /// Current width and height of the atlas texture.
    pub fn size(&self) -> u32 {
        self.size
    }

    /// Starts a frame. Glyphs used by any layout prepared in the same frame are
    /// not evicted for each other.
    pub fn begin_frame(&mut self) -> u64 {
        let previous = self.frame;
        // Retry suppressed layouts when another layout stops pinning glyphs.
        if self.glyphs.values().any(|glyph| {
            matches!(glyph.image, AtlasImage::Placed(_)) && glyph.last_used + 1 == previous
        }) {
            self.availability += 1;
        }
        self.glyphs.retain(|_, glyph| {
            glyph.last_used == previous || matches!(glyph.image, AtlasImage::Placed(_))
        });
        self.layouts_this_frame = 0;
        self.stale = None;
        self.frame += 1;
        self.frame
    }

    /// Marks the uploads of `frame` as written to the GPU texture.
    pub fn commit(&mut self, frame: u64) {
        for glyph in self.glyphs.values_mut() {
            if glyph.upload_frame == frame {
                glyph.resident = true;
            }
        }
    }

    /// Whether `key` holds a marker of the given kind, which routes the keys it is read for.
    /// It counts as used this frame.
    fn mark_used(&mut self, key: &AtlasKey, oversized: bool) -> bool {
        let Some(glyph) = self.glyphs.get_mut(key) else {
            return false;
        };
        let marker = match glyph.image {
            AtlasImage::Oversized => oversized,
            AtlasImage::RasterOnly => !oversized,
            _ => false,
        };
        if marker {
            glyph.last_used = self.frame;
        }
        marker
    }

    /// Distance fields are used where requested, and for raster glyphs too
    /// large for the atlas, unless the glyph can only be rasterized.
    fn key_for(&mut self, key: GlyphKey, mode: TextRenderMode) -> AtlasKey {
        let raster = AtlasKey::Raster(key.0);
        let oversized = self.mark_used(&raster, true);
        let msdf = AtlasKey::Msdf(msdf::msdf_key(key.0));
        let wanted = match mode {
            TextRenderMode::Raster => false,
            TextRenderMode::Msdf => true,
            TextRenderMode::Auto => f32::from_bits(key.0.font_size_bits) >= MSDF_AUTO_MIN_SIZE,
        } || oversized;
        if wanted && !self.mark_used(&msdf, false) {
            return msdf;
        }
        raster
    }

    /// The key of the stroke `half_width` pixels to each side of `key`'s outline: a field
    /// where the glyph draws one, or where its stroked raster outline is too large for the
    /// atlas, unless the outline can only be rasterized.
    fn stroke_key_for(&mut self, key: GlyphKey, mode: TextRenderMode, half_width: f32) -> AtlasKey {
        let raster = AtlasKey::RasterStroke(key.0, stroke_width_key(half_width));
        let oversized = self.mark_used(&raster, true);
        let (field, tier) = msdf::stroke_key(key.0, half_width);
        let field = AtlasKey::Sdf(field, tier);
        let wanted = matches!(self.key_for(key, mode), AtlasKey::Msdf(_)) || oversized;
        if wanted && !self.mark_used(&field, false) {
            return field;
        }
        raster
    }

    /// Unique atlas keys of `glyphs` and their `strokes`, in order. Markers that route the keys
    /// count as used this frame.
    fn keys_for(
        &mut self,
        glyphs: &[TextGlyph],
        mode: TextRenderMode,
        strokes: &[f32],
    ) -> Vec<AtlasKey> {
        let mut seen = foldhash::HashSet::default();
        let mut keys = Vec::new();
        for glyph in glyphs {
            let stroke = glyph_stroke(strokes, glyph)
                .map(|half_width| self.stroke_key_for(glyph.key, mode, half_width));
            for key in [Some(self.key_for(glyph.key, mode)), stroke]
                .into_iter()
                .flatten()
            {
                if seen.insert(key) {
                    keys.push(key);
                }
            }
        }
        keys
    }

    fn touch(&mut self, keys: &[AtlasKey]) {
        for key in keys {
            if let Some(glyph) = self.glyphs.get_mut(key) {
                glyph.last_used = self.frame;
            }
        }
    }

    fn clear(&mut self) {
        self.glyphs.clear();
        self.allocator.clear();
        self.allocations = 0;
        self.availability += 1;
        self.stale = None;
    }

    fn insert(&mut self, key: AtlasKey, image: AtlasImage) {
        if matches!(image, AtlasImage::Oversized | AtlasImage::RasterOnly) {
            self.markers += 1;
        }
        self.glyphs.insert(
            key,
            AtlasGlyph {
                image,
                last_used: self.frame,
                upload_frame: self.frame,
                resident: !matches!(image, AtlasImage::Placed(_)),
            },
        );
    }

    /// Allocates a slot, evicting the least recently used glyphs not used this
    /// frame.
    fn allocate(&mut self, size: [u32; 2]) -> Option<(AllocId, [u32; 2])> {
        let request = size2(size[0].max(1) as i32, size[1].max(1) as i32);
        loop {
            if self.allocations < MAX_ATLAS_GLYPHS
                && let Some(allocation) = self.allocator.allocate(request)
            {
                self.allocations += 1;
                let min = allocation.rectangle.min;
                return Some((allocation.id, [min.x as u32, min.y as u32]));
            }
            if self.stale.is_none() {
                self.stale = Some(self.stale_by_age());
            }
            let key = self.stale.as_mut()?.pop()?;
            // Layouts prepared since the candidates were collected may use it.
            if let Some(AtlasGlyph {
                image: AtlasImage::Placed(placed),
                last_used,
                ..
            }) = self.glyphs.get(&key)
                && *last_used != self.frame
            {
                self.allocator.deallocate(placed.slot);
                self.glyphs.remove(&key);
                self.allocations -= 1;
                self.availability += 1;
            }
        }
    }

    /// Placed glyphs unused this frame, most recently used first.
    fn stale_by_age(&self) -> Vec<AtlasKey> {
        let mut stale: Vec<(u64, AtlasKey)> = self
            .glyphs
            .iter()
            .filter(|(_, glyph)| {
                matches!(glyph.image, AtlasImage::Placed(_)) && glyph.last_used != self.frame
            })
            .map(|(key, glyph)| (glyph.last_used, *key))
            .collect();
        stale.sort_unstable_by_key(|(last_used, _)| std::cmp::Reverse(*last_used));
        stale.into_iter().map(|(_, key)| key).collect()
    }
}

impl PlacedGlyph {
    /// The instance that draws `glyph` with this image, placed under `key`.
    fn instance(&self, glyph: &TextGlyph, key: AtlasKey) -> GpuGlyphInstance {
        let [width, height] = self.size.map(|side| side as f32);
        let [x, y] = self.origin.map(|texel| texel as f32);
        let (rect, uv_rect) = if self.mode == GLYPH_MODE_MSDF {
            // Fields are scaled to the glyph's size at its exact position, and
            // inset by half a texel so filtering stays inside the slot.
            let font_size = f32::from_bits(glyph.key.0.font_size_bits);
            let scale = font_size / f32::from_bits(key.cache_key().font_size_bits);
            (
                [
                    glyph.position[0] + (self.left + 0.5) * scale,
                    glyph.position[1] - (self.top - 0.5) * scale,
                    (width - 1.0) * scale,
                    (height - 1.0) * scale,
                ],
                [x + 0.5, y + 0.5, x + width - 0.5, y + height - 0.5],
            )
        } else {
            (
                [glyph.x + self.left, glyph.y - self.top, width, height],
                [x, y, x + width, y + height],
            )
        };
        let flags = match key {
            AtlasKey::Sdf(_, tier) => tier << GLYPH_RANGE_SHIFT,
            _ => 0,
        };
        GpuGlyphInstance {
            rect,
            uv_rect,
            opacity: 1.0,
            partner: 0,
            _padding: [0; 2],
            mode: self.mode,
            flags,
            offset: [0.0; 2],
            linear: [[1.0, 0.0], [0.0, 1.0]],
        }
    }
}

pub(crate) enum RenderedGlyph {
    Pixels(GlyphPixels),
    Empty,
    Oversized,
    RasterOnly,
}

pub(crate) struct GlyphPixels {
    pub(crate) size: [u32; 2],
    pub(crate) left: f32,
    pub(crate) top: f32,
    pub(crate) mode: u32,
    /// Premultiplied RGBA8 rows; distance fields keep their channels in RGB.
    pub(crate) pixels: Vec<u8>,
}

fn raster_pixels(image: &SwashImage, key: CacheKey) -> GlyphPixels {
    // Swash composites colour outlines premultiplied but decodes colour
    // bitmaps with straight alpha.
    let straight_color = matches!(image.source, Source::ColorBitmap(_));
    let texels = image.placement.width as usize * image.placement.height as usize;
    let pixels = (0..texels)
        .flat_map(|index| match image.content {
            SwashContent::Mask => [image.data[index]; 4],
            SwashContent::SubpixelMask => {
                let rgb = &image.data[index * 4..index * 4 + 3];
                [rgb[0].max(rgb[1]).max(rgb[2]); 4]
            }
            SwashContent::Color => {
                let [r, g, b, a] = image.data[index * 4..index * 4 + 4] else {
                    unreachable!()
                };
                if straight_color {
                    [premultiply(r, a), premultiply(g, a), premultiply(b, a), a]
                } else {
                    [r, g, b, a]
                }
            }
        })
        .collect();
    // Swash draws bitmaps without the key's subpixel bins, so they are
    // shifted to the nearest whole pixel instead.
    let [snap_x, snap_y] = if matches!(image.source, Source::Bitmap(_) | Source::ColorBitmap(_)) {
        [key.x_bin, key.y_bin].map(|bin| bin.as_float().round())
    } else {
        [0.0; 2]
    };
    GlyphPixels {
        size: [image.placement.width, image.placement.height],
        left: image.placement.left as f32 + snap_x,
        top: image.placement.top as f32 + snap_y,
        mode: if image.content == SwashContent::Color {
            GLYPH_MODE_RASTER_COLOR
        } else {
            GLYPH_MODE_RASTER_MASK
        },
        pixels,
    }
}

fn premultiply(channel: u8, alpha: u8) -> u8 {
    ((u32::from(channel) * u32::from(alpha) + 127) / 255) as u8
}

/// Glyph pixels to write into the atlas texture.
#[derive(Debug, Clone)]
pub struct GlyphUpload {
    pub origin: [u32; 2],
    pub size: [u32; 2],
    /// Tightly packed RGBA8 rows.
    pub pixels: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct PreparedGlyphs {
    pub instances: Vec<GpuGlyphInstance>,
    /// Index in the layout's glyphs of each instance.
    pub sources: Vec<usize>,
    /// Instances that stroke glyph outlines.
    pub strokes: Vec<GpuGlyphInstance>,
    /// Index in `instances` of the glyph each of `strokes` strokes.
    pub stroke_glyphs: Vec<usize>,
    pub uploads: Vec<GlyphUpload>,
    /// Glyph and stroke instances not drawn because the atlas is full.
    pub dropped_glyphs: usize,
}

#[cfg(test)]
#[path = "atlas_tests.rs"]
mod tests;
