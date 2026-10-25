//! Text layout and GPU-facing text data for Lumen.
//!
//! This crate deliberately owns text shaping, measurement, atlas bookkeeping,
//! and WGSL-facing buffer formats. Lumen nodes should depend on this layer
//! rather than coupling directly to a specific renderer such as glyphon.

use std::collections::{HashMap, HashSet, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};

use cosmic_text::{
    Align, Attrs, Buffer, CacheKey, CacheKeyFlags, Family, FontSystem, LayoutGlyph, Metrics,
    Shaping, Style, SwashCache, SwashContent, SwashImage, Weight, Wrap,
};
use etagere::{AllocId, size2};
use swash::scale::Source;

mod gpu;
#[cfg(feature = "msdf")]
mod msdf;
pub use gpu::*;

/// Text shader body; the engine prepends its alpha and paint WGSL chunks.
pub const TEXT_SHADER: &str = include_str!("shaders/text.wgsl");
pub const DEFAULT_FONT_FAMILY: &str = "Roboto";
pub const ROBOTO_REGULAR_BYTES: &[u8] = include_bytes!("../assets/roboto/Roboto-Regular.ttf");
/// Whether [`TextRenderMode::Msdf`] is compiled in.
pub const MSDF_SUPPORTED: bool = cfg!(feature = "msdf");
/// Em size, in atlas texels, that every MSDF glyph is generated at.
pub const MSDF_EM_SIZE: f32 = 48.0;
/// Width of the MSDF distance range in atlas texels, centred on the outline.
pub const MSDF_PX_RANGE: f32 = 6.0;

/// How outline glyphs are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TextRenderMode {
    /// Glyphs are rasterized for their size and subpixel position.
    #[default]
    Raster,
    /// Outline glyphs share one distance field across every size. Colour,
    /// bitmap and overlapping-contour glyphs, and every glyph in builds without
    /// the `msdf` feature, are rasterized.
    Msdf,
}

#[derive(Debug)]
pub struct TextSystem {
    font_system: FontSystem,
    swash_cache: SwashCache,
    font_generation: u64,
}

impl TextSystem {
    pub fn new() -> Self {
        let mut font_system = FontSystem::new_with_fonts(std::iter::empty());
        load_default_fonts(&mut font_system);
        Self {
            font_system,
            swash_cache: SwashCache::new(),
            font_generation: 0,
        }
    }

    pub fn load_font_data(&mut self, data: Vec<u8>) {
        self.font_system.db_mut().load_font_data(data);
        self.font_generation += 1;
    }

    /// Changes whenever fonts are loaded, so shaped text can be invalidated.
    pub fn font_generation(&self) -> u64 {
        self.font_generation
    }

    pub fn measure(&mut self, request: &TextLayoutRequest) -> TextMeasurement {
        self.shape(request).measurement
    }

    pub fn layout(&mut self, request: &TextLayoutRequest) -> TextLayout {
        self.shape(request).place(request.origin, request.color)
    }

    /// Shapes text without placing it, so it can be placed at many origins.
    pub fn shape(&mut self, request: &TextLayoutRequest) -> ShapedText {
        let metrics = Metrics::new(request.font_size.max(1.0), request.line_height());
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_size(request.max_width.filter(|width| *width > 0.0), None);
        buffer.set_wrap(if request.max_width.is_some_and(|width| width > 0.0) {
            Wrap::WordOrGlyph
        } else {
            Wrap::None
        });
        let attrs = Attrs::new()
            .family(Family::Name(&request.font_family))
            .weight(Weight(request.font_weight.clamp(1, 1000)))
            .style(match request.font_style {
                TextFontStyle::Italic => Style::Italic,
                TextFontStyle::Oblique => Style::Oblique,
                TextFontStyle::Normal => Style::Normal,
            })
            .cache_key_flags(if request.disable_hinting {
                CacheKeyFlags::DISABLE_HINTING
            } else {
                CacheKeyFlags::empty()
            });
        buffer.set_text(
            &request.content,
            &attrs,
            Shaping::Advanced,
            Some(match request.align {
                TextAlign::Left => Align::Left,
                TextAlign::Center => Align::Center,
                TextAlign::Right => Align::Right,
                TextAlign::Justified => Align::Justified,
            }),
        );
        buffer.shape_until_scroll(&mut self.font_system, true);

        let mut glyphs = Vec::new();
        let mut width: f32 = 0.0;
        let mut height: f32 = 0.0;
        let mut x_min = f32::INFINITY;
        let mut x_max = f32::NEG_INFINITY;
        for run in buffer.layout_runs() {
            width = width.max(run.line_w);
            height = height.max(run.line_top + run.line_height);
            for glyph in run.glyphs {
                x_min = x_min.min(glyph.x);
                x_max = x_max.max(glyph.x + glyph.w);
                glyphs.push(ShapedGlyph {
                    glyph: glyph.clone(),
                    line_y: run.line_y,
                });
            }
        }
        let bounds = if glyphs.is_empty() {
            [0.0, 0.0, 0.0, height]
        } else {
            [x_min, 0.0, x_max - x_min, height]
        };

        ShapedText {
            measurement: TextMeasurement { width, height },
            bounds,
            glyphs,
        }
    }

    /// Draws `layout` into a fresh `size`² atlas and returns its pixels.
    #[cfg(test)]
    fn render_atlas(
        &mut self,
        layout: &TextLayout,
        mode: TextRenderMode,
        size: u32,
        max_glyphs: usize,
    ) -> AtlasRender {
        let mut atlas = GlyphAtlas::new(size);
        atlas.begin_frame();
        let prepared = self.prepare_glyphs(&mut atlas, layout, mode, max_glyphs, &mut None);
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

    /// Places `layout` in `atlas` for its current frame, generating glyphs that
    /// are not resident yet one at a time.
    ///
    /// When the atlas is full, the least recently used glyphs that no layout
    /// used this frame are evicted. If there is still no room and only this
    /// layout uses the atlas this frame, the atlas is cleared and repacked.
    /// Glyphs that still do not fit, and glyphs past `max_glyphs`, are dropped.
    /// `overfull` suppresses retries until the working set or reclaimable space
    /// changes. Uploads become resident after [`GlyphAtlas::commit`].
    pub fn prepare_glyphs(
        &mut self,
        atlas: &mut GlyphAtlas,
        layout: &TextLayout,
        mode: TextRenderMode,
        max_glyphs: usize,
        overfull: &mut Option<u64>,
    ) -> PreparedGlyphs {
        let visible = &layout.glyphs[..layout.glyphs.len().min(max_glyphs)];
        let working_set = {
            let mut keys: Vec<GlyphKey> = visible.iter().map(|glyph| glyph.key).collect();
            keys.sort_unstable();
            keys.dedup();
            let mut hasher = DefaultHasher::new();
            (keys, mode).hash(&mut hasher);
            hasher.finish()
        };
        let overflow_key = |availability| {
            let mut hasher = DefaultHasher::new();
            (working_set, availability).hash(&mut hasher);
            hasher.finish()
        };
        let retry_new = *overfull != Some(overflow_key(atlas.availability));
        let mut uploads = Vec::new();
        let compacted = !self.place_glyphs(atlas, visible, mode, retry_new, &mut uploads)
            && atlas.layouts_this_frame == 0;
        if compacted {
            atlas.clear();
            uploads.clear();
            self.place_glyphs(atlas, visible, mode, true, &mut uploads);
        }

        let mut instances = Vec::with_capacity(visible.len());
        let mut capacity_dropped = 0;
        for glyph in visible {
            match atlas.glyphs.get(&atlas.key_for(glyph.key, mode)) {
                Some(AtlasGlyph {
                    image: AtlasImage::Placed(placed),
                    ..
                }) => instances.push(placed.instance(glyph)),
                Some(AtlasGlyph {
                    image: AtlasImage::Empty,
                    ..
                }) => {}
                _ => capacity_dropped += 1,
            }
        }
        *overfull = (capacity_dropped > 0).then(|| overflow_key(atlas.availability));

        atlas.layouts_this_frame += 1;
        PreparedGlyphs {
            frame: atlas.frame,
            instances,
            uploads,
            dropped_glyphs: layout.glyphs.len() - visible.len() + capacity_dropped,
            compacted,
        }
    }

    /// Returns `false` when the atlas ran out of room.
    fn place_glyphs(
        &mut self,
        atlas: &mut GlyphAtlas,
        glyphs: &[TextGlyph],
        mode: TextRenderMode,
        retry_new: bool,
        uploads: &mut Vec<GlyphUpload>,
    ) -> bool {
        // A second pass places raster glyphs for distance fields that turned
        // out to be raster-only.
        loop {
            let keys = atlas.keys_for(glyphs, mode);
            atlas.touch(&keys);
            if !self.place_missing(atlas, &keys, retry_new, uploads) {
                return false;
            }
            if atlas.keys_for(glyphs, mode) == keys {
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
        let mut stale = None;
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
                    let Some((slot, origin)) = atlas.allocate(pixels.size, &mut stale) else {
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
                let plain_outline = self
                    .font_system
                    .get_font(key.font_id, key.font_weight)
                    .is_some_and(|font| {
                        ![b"COLR", b"CBDT", b"sbix", b"EBDT"]
                            .iter()
                            .any(|tag| font.as_swash().table(u32::from_be_bytes(**tag)).is_some())
                    });
                if plain_outline
                    && let Some(commands) = self
                        .swash_cache
                        .get_outline_commands_uncached(&mut self.font_system, key)
                {
                    let mut oversized = false;
                    swash::zeno::Mask::new(&commands[..])
                        .origin(swash::zeno::Origin::BottomLeft)
                        .offset(swash::zeno::Vector::new(
                            key.x_bin.as_float(),
                            key.y_bin.as_float(),
                        ))
                        .inspect(|_, width, height| {
                            oversized = width > max_size || height > max_size
                        });
                    if oversized {
                        return RenderedGlyph::Oversized;
                    }
                }
                self.swash_cache
                    .get_image_uncached(&mut self.font_system, key)
                    .filter(|image| image.placement.width > 0 && image.placement.height > 0)
                    .map_or(RenderedGlyph::Empty, |image| {
                        RenderedGlyph::Pixels(raster_pixels(&image, key))
                    })
            }
            #[cfg(feature = "msdf")]
            AtlasKey::Msdf(key) => {
                msdf::render_msdf_glyph(&mut self.font_system, &mut self.swash_cache, key)
            }
        }
    }
}

impl Default for TextSystem {
    fn default() -> Self {
        Self::new()
    }
}

fn load_default_fonts(font_system: &mut FontSystem) {
    let db = font_system.db_mut();
    db.load_font_data(ROBOTO_REGULAR_BYTES.to_vec());
    db.set_sans_serif_family(DEFAULT_FONT_FAMILY);
}

#[derive(Debug, Clone)]
pub struct TextLayoutRequest {
    pub content: String,
    pub font_family: String,
    pub font_size: f32,
    pub font_weight: u16,
    pub font_style: TextFontStyle,
    pub max_width: Option<f32>,
    pub line_height: Option<f32>,
    pub align: TextAlign,
    pub origin: [f32; 2],
    pub color: [f32; 4],
    pub disable_hinting: bool,
}

impl TextLayoutRequest {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            font_family: DEFAULT_FONT_FAMILY.to_string(),
            font_size: 16.0,
            font_weight: 400,
            font_style: TextFontStyle::Normal,
            max_width: None,
            line_height: None,
            align: TextAlign::Left,
            origin: [0.0, 0.0],
            color: [1.0, 1.0, 1.0, 1.0],
            disable_hinting: true,
        }
    }

    pub fn line_height(&self) -> f32 {
        self.line_height.unwrap_or(self.font_size * 1.2).max(1.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextFontStyle {
    Normal,
    Italic,
    Oblique,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
    Justified,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextMeasurement {
    pub width: f32,
    pub height: f32,
}

/// Shaped text relative to a `[0, 0]` origin.
#[derive(Debug, Clone)]
pub struct ShapedText {
    pub measurement: TextMeasurement,
    /// Advance box of all glyphs as `[x, y, width, height]`.
    pub bounds: [f32; 4],
    glyphs: Vec<ShapedGlyph>,
}

#[derive(Debug, Clone)]
struct ShapedGlyph {
    glyph: LayoutGlyph,
    line_y: f32,
}

impl ShapedText {
    /// Places glyphs at `origin`. The fractional origin selects each glyph's
    /// subpixel bins, and raster glyph positions are snapped to whole pixels.
    pub fn place(&self, origin: [f32; 2], color: [f32; 4]) -> TextLayout {
        let glyphs = self
            .glyphs
            .iter()
            .map(|shaped| {
                let glyph = &shaped.glyph;
                let position = [
                    glyph.x + glyph.font_size * glyph.x_offset + origin[0],
                    glyph.y - glyph.font_size * glyph.y_offset + (origin[1] + shaped.line_y),
                ];
                // Swash offsets outlines with Y pointing up; layout positions point down.
                let (cache_key, x, y) = CacheKey::new(
                    glyph.font_id,
                    glyph.glyph_id,
                    glyph.font_size,
                    (position[0], -position[1]),
                    glyph.font_weight,
                    glyph.cache_key_flags,
                );
                TextGlyph {
                    key: GlyphKey(cache_key),
                    x: x as f32,
                    y: -y as f32,
                    position,
                    color,
                }
            })
            .collect();
        TextLayout {
            measurement: self.measurement,
            bounds: [
                origin[0] + self.bounds[0],
                origin[1] + self.bounds[1],
                self.bounds[2],
                self.bounds[3],
            ],
            glyphs,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextLayout {
    pub measurement: TextMeasurement,
    /// Advance box of all glyphs as `[x, y, width, height]`.
    pub bounds: [f32; 4],
    pub glyphs: Vec<TextGlyph>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextGlyph {
    /// Raster key, including the glyph's subpixel bin.
    pub key: GlyphKey,
    /// Whole-pixel pen position; the subpixel remainder is carried by `key`.
    pub x: f32,
    pub y: f32,
    /// Exact pen position, used by distance-field glyphs.
    pub position: [f32; 2],
    pub color: [f32; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct GlyphKey(pub CacheKey);

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
    glyphs: HashMap<AtlasKey, AtlasGlyph>,
    frame: u64,
    layouts_this_frame: usize,
}

/// Raster glyphs are keyed by size and subpixel bin; distance fields only by
/// font, glyph, weight and fake italic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum AtlasKey {
    Raster(CacheKey),
    #[cfg(feature = "msdf")]
    Msdf(CacheKey),
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
    /// Image offset from the pen, y up; in pixels for raster glyphs and
    /// [`MSDF_EM_SIZE`] units for distance fields.
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
            glyphs: HashMap::new(),
            frame: 0,
            layouts_this_frame: 0,
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
        self.glyphs.retain(|key, glyph| {
            glyph.last_used == previous
                || !matches!(
                    (key, glyph.image),
                    (
                        AtlasKey::Raster(_),
                        AtlasImage::Empty | AtlasImage::Oversized
                    )
                )
        });
        self.layouts_this_frame = 0;
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

    /// Number of cached glyphs, including empty ones.
    pub fn len(&self) -> usize {
        self.glyphs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }

    fn key_for(&self, key: GlyphKey, mode: TextRenderMode) -> AtlasKey {
        #[cfg(feature = "msdf")]
        if mode == TextRenderMode::Msdf {
            let msdf = AtlasKey::Msdf(msdf::msdf_key(key.0));
            if !self
                .glyphs
                .get(&msdf)
                .is_some_and(|glyph| matches!(glyph.image, AtlasImage::RasterOnly))
            {
                return msdf;
            }
        }
        #[cfg(not(feature = "msdf"))]
        let _ = mode;
        AtlasKey::Raster(key.0)
    }

    /// Unique atlas keys of `glyphs`, in order.
    fn keys_for(&self, glyphs: &[TextGlyph], mode: TextRenderMode) -> Vec<AtlasKey> {
        let mut seen = HashSet::new();
        glyphs
            .iter()
            .map(|glyph| self.key_for(glyph.key, mode))
            .filter(|key| seen.insert(*key))
            .collect()
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
    }

    fn insert(&mut self, key: AtlasKey, image: AtlasImage) {
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
    /// frame. `stale` caches the eviction order.
    fn allocate(
        &mut self,
        size: [u32; 2],
        stale: &mut Option<Vec<AtlasKey>>,
    ) -> Option<(AllocId, [u32; 2])> {
        let request = size2(size[0].max(1) as i32, size[1].max(1) as i32);
        loop {
            if self.allocations < MAX_ATLAS_GLYPHS
                && let Some(allocation) = self.allocator.allocate(request)
            {
                self.allocations += 1;
                let min = allocation.rectangle.min;
                return Some((allocation.id, [min.x as u32, min.y as u32]));
            }
            let stale = stale.get_or_insert_with(|| self.stale_by_age());
            if let Some(key) = stale.pop() {
                if let Some(AtlasGlyph {
                    image: AtlasImage::Placed(placed),
                    ..
                }) = self.glyphs.remove(&key)
                {
                    self.allocator.deallocate(placed.slot);
                    self.allocations -= 1;
                    self.availability += 1;
                }
            } else {
                return None;
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
    fn instance(&self, glyph: &TextGlyph) -> GpuGlyphInstance {
        let [width, height] = self.size.map(|side| side as f32);
        let [x, y] = self.origin.map(|texel| texel as f32);
        let (rect, uv_rect) = if self.mode == GLYPH_MODE_MSDF {
            // Fields are scaled to the glyph's size at its exact position, and
            // inset by half a texel so filtering stays inside the slot.
            let scale = f32::from_bits(glyph.key.0.font_size_bits) / MSDF_EM_SIZE;
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
        GpuGlyphInstance {
            rect,
            uv_rect,
            color: glyph.color,
            mode: self.mode,
            _padding: [0; 3],
        }
    }
}

enum RenderedGlyph {
    Pixels(GlyphPixels),
    Empty,
    Oversized,
    #[cfg_attr(not(feature = "msdf"), allow(dead_code))]
    RasterOnly,
}

struct GlyphPixels {
    size: [u32; 2],
    left: f32,
    top: f32,
    mode: u32,
    /// Premultiplied RGBA8 rows; distance fields keep their channels in RGB.
    pixels: Vec<u8>,
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
    /// Pass to [`GlyphAtlas::commit`] once `uploads` have been written.
    pub frame: u64,
    pub instances: Vec<GpuGlyphInstance>,
    pub uploads: Vec<GlyphUpload>,
    /// Glyphs not drawn because of `max_glyphs` or atlas capacity.
    pub dropped_glyphs: usize,
    /// The atlas was cleared and repacked this frame.
    pub compacted: bool,
}

#[cfg(test)]
#[derive(Debug, Clone)]
struct AtlasRender {
    pixels: Vec<u8>,
    instances: Vec<GpuGlyphInstance>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prepare(
        system: &mut TextSystem,
        atlas: &mut GlyphAtlas,
        layout: &TextLayout,
        mode: TextRenderMode,
    ) -> PreparedGlyphs {
        system.prepare_glyphs(atlas, layout, mode, 4096, &mut None)
    }

    #[test]
    fn measures_roboto_text() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("External URL media");
        request.font_size = 44.0;
        request.max_width = Some(900.0);

        let measurement = system.measure(&request);

        assert!(measurement.width > 100.0);
        assert!(measurement.height > 40.0);
    }

    #[test]
    fn wraps_text_to_max_width() {
        let mut system = TextSystem::new();
        let mut unwrapped = TextLayoutRequest::new("one two three four five");
        unwrapped.font_size = 24.0;
        let mut wrapped = unwrapped.clone();
        wrapped.max_width = Some(80.0);

        let wide = system.measure(&unwrapped);
        let narrow = system.measure(&wrapped);

        assert!(wide.width > narrow.width);
        assert!(narrow.height > wide.height);
    }

    #[test]
    fn atlas_contains_glyph_pixels() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("A");
        request.font_size = 48.0;
        let layout = system.layout(&request);
        let render = system.render_atlas(&layout, TextRenderMode::Raster, 128, 16);

        assert_eq!(render.instances.len(), 1);
        let instance = render.instances[0];
        assert!(instance.rect[2] > 0.0 && instance.uv_rect[2] > instance.uv_rect[0]);
        assert!(render.pixels.chunks_exact(4).any(|pixel| pixel[3] > 0));
    }

    #[test]
    fn colour_glyphs_are_stored_premultiplied() {
        let mut image = SwashImage::new();
        image.content = SwashContent::Color;
        image.placement.width = 2;
        image.placement.height = 1;
        image.data = vec![255, 0, 0, 128, 0, 100, 200, 255];
        let key = TextSystem::new()
            .layout(&TextLayoutRequest::new("A"))
            .glyphs[0]
            .key
            .0;

        image.source = Source::ColorBitmap(swash::scale::StrikeWith::BestFit);
        let bitmap = raster_pixels(&image, key);
        image.source = Source::ColorOutline(0);
        let outline = raster_pixels(&image, key);

        assert_eq!(bitmap.pixels, [128, 0, 0, 128, 0, 100, 200, 255]);
        assert_eq!(outline.pixels, image.data);
        assert_eq!(bitmap.mode, GLYPH_MODE_RASTER_COLOR);
    }

    #[test]
    fn bitmap_glyphs_snap_to_the_nearest_pixel() {
        let mut system = TextSystem::new();
        let shaped = system.shape(&TextLayoutRequest::new("A"));
        let mut image = SwashImage::new();
        image.source = Source::ColorBitmap(swash::scale::StrikeWith::BestFit);
        image.content = SwashContent::Color;
        image.placement = swash::zeno::Placement {
            left: 1,
            top: 8,
            width: 2,
            height: 2,
        };
        image.data = vec![0; 2 * 2 * 4];
        let mut atlas = GlyphAtlas::new(64);
        let (slot, origin) = atlas.allocate([2, 2], &mut None).unwrap();
        for start in [20.0, -21.0] {
            let mut previous = f32::NEG_INFINITY;
            for step in 0..=8 {
                let y = start + step as f32 * 0.125;
                let layout = shaped.place([3.3, y], [1.0; 4]);
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
                let rendered_y = placed.instance(glyph).rect[1] + 8.0;
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
        let template = system.layout(&TextLayoutRequest::new("A")).glyphs[0].key.0;
        let mut atlas = GlyphAtlas::new(512);
        let mut stale = None;
        for index in 0..70_000_u32 {
            if index % 1000 == 0 {
                atlas.begin_frame();
                stale = None;
            }
            let (slot, origin) = atlas.allocate([1, 1], &mut stale).unwrap();
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
        assert_eq!(atlas.len(), MAX_ATLAS_GLYPHS);
    }

    #[test]
    fn raster_glyphs_are_placed_on_whole_pixels() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("AV Sharp text\nمرحبا بالعالم wraps here");
        request.font_size = 31.0;
        request.max_width = Some(220.0);
        request.align = TextAlign::Center;
        let shaped = system.shape(&request);

        let first = shaped.place([10.1, 5.3], [1.0; 4]);
        let second = shaped.place([10.6, 5.3], [1.0; 4]);
        let render = system.render_atlas(&second, TextRenderMode::Raster, 1024, 128);

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
    fn fractional_y_changes_raster_subpixel_bin_and_pixels() {
        let mut system = TextSystem::new();
        let shaped = system.shape(&TextLayoutRequest::new("A"));
        let first = shaped.place([0.0, 0.0], [1.0; 4]);
        let second = shaped.place([0.0, 0.5], [1.0; 4]);

        assert_ne!(first.glyphs[0].key.0.y_bin, second.glyphs[0].key.0.y_bin);
        let first = system.render_atlas(&first, TextRenderMode::Raster, 64, 128);
        let second = system.render_atlas(&second, TextRenderMode::Raster, 64, 128);
        assert_ne!(first.pixels, second.pixels);
    }

    #[test]
    fn raster_ink_follows_fractional_y() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("H");
        request.font_size = 24.0;
        let shaped = system.shape(&request);
        let size = 64;
        let ink_center = |system: &mut TextSystem, y: f32| {
            let layout = shaped.place([4.0, y], [1.0; 4]);
            let render = system.render_atlas(&layout, TextRenderMode::Raster, size, 8);
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
        request.font_size = 40.0;
        let mut layout = system.layout(&request);
        atlas.begin_frame();
        let full = system.prepare_glyphs(
            &mut atlas,
            &layout,
            TextRenderMode::Raster,
            128,
            &mut overfull,
        );
        atlas.commit(full.frame);
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
            128,
            &mut overfull,
        );
        assert_eq!(recovered.dropped_glyphs, 0);
        assert!(!recovered.instances.is_empty());
        assert!(!recovered.uploads.is_empty());
    }

    #[test]
    fn overflowed_layout_recovers_when_other_layout_becomes_stale() {
        let mut system = TextSystem::new();
        let mut atlas = GlyphAtlas::new(64);
        let mut first = TextLayoutRequest::new("W");
        first.font_size = 64.0;
        let first = system.layout(&first);
        let mut second = TextLayoutRequest::new("A");
        second.font_size = 40.0;
        let second = system.layout(&second);
        let mut overfull = None;
        atlas.begin_frame();
        let placed = prepare(&mut system, &mut atlas, &first, TextRenderMode::Raster);
        assert!(!placed.instances.is_empty());
        let blocked = system.prepare_glyphs(
            &mut atlas,
            &second,
            TextRenderMode::Raster,
            128,
            &mut overfull,
        );
        assert!(blocked.dropped_glyphs > 0);
        atlas.commit(blocked.frame);
        atlas.begin_frame();
        let suppressed = system.prepare_glyphs(
            &mut atlas,
            &second,
            TextRenderMode::Raster,
            128,
            &mut overfull,
        );
        assert!(suppressed.uploads.is_empty());
        atlas.begin_frame();
        let recovered = system.prepare_glyphs(
            &mut atlas,
            &second,
            TextRenderMode::Raster,
            128,
            &mut overfull,
        );
        assert_eq!(recovered.dropped_glyphs, 0);
        assert!(!recovered.uploads.is_empty());
    }

    #[test]
    fn replacing_one_glyph_in_full_atlas_evicts_only_stale_glyph() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("ABCDE");
        request.font_size = 24.0;
        let mut layout = system.layout(&request);
        let mut atlas = GlyphAtlas::new(64);
        atlas.begin_frame();
        let mut origins = Vec::new();
        for glyph in &layout.glyphs[..4] {
            let (slot, origin) = atlas.allocate([32, 32], &mut None).unwrap();
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
        assert!(atlas.allocate([1, 1], &mut Some(Vec::new())).is_none());
        let replaced_key = AtlasKey::Raster(layout.glyphs.remove(3).key.0);

        atlas.begin_frame();
        let replaced = prepare(&mut system, &mut atlas, &layout, TextRenderMode::Raster);
        assert_eq!(replaced.dropped_glyphs, 0);
        assert!(!replaced.compacted);
        assert!(!atlas.glyphs.contains_key(&replaced_key));
        for (glyph, origin) in layout.glyphs.iter().zip(origins).take(3) {
            let AtlasImage::Placed(placed) = atlas.glyphs[&AtlasKey::Raster(glyph.key.0)].image
            else {
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
            request.font_size = 32.0 + frame as f32 * (568.0 / 59.0);
            let layout = system.layout(&request);
            seen.extend(layout.glyphs.iter().map(|glyph| glyph.key));
            atlas.begin_frame();
            let prepared = prepare(&mut system, &mut atlas, &layout, TextRenderMode::Raster);
            atlas.commit(prepared.frame);
            assert_eq!(prepared.dropped_glyphs, 0, "frame {frame}");
        }

        assert!(atlas.len() < seen.len() / 2);
        assert!(system.swash_cache.image_cache.is_empty());
    }

    #[test]
    fn uncommitted_uploads_are_repeated() {
        let mut system = TextSystem::new();
        let mut atlas = GlyphAtlas::new(256);
        let layout = system.layout(&TextLayoutRequest::new("Retry"));

        atlas.begin_frame();
        let dropped = prepare(&mut system, &mut atlas, &layout, TextRenderMode::Raster);
        atlas.begin_frame();
        let retried = prepare(&mut system, &mut atlas, &layout, TextRenderMode::Raster);
        atlas.commit(retried.frame);
        atlas.begin_frame();
        let committed = prepare(&mut system, &mut atlas, &layout, TextRenderMode::Raster);

        assert!(!dropped.uploads.is_empty());
        assert_eq!(retried.uploads.len(), dropped.uploads.len());
        assert_eq!(retried.uploads[0].origin, dropped.uploads[0].origin);
        assert!(committed.uploads.is_empty());
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
            request.font_size = 28.0;
            request.max_width = Some(900.0);
            system.layout(&request)
        };
        let first = layout(&mut system, first);
        let second = layout(&mut system, second);

        for mode in [TextRenderMode::Raster, TextRenderMode::Msdf] {
            let frame = atlas.begin_frame();
            let a = prepare(&mut system, &mut atlas, &first, mode);
            let b = prepare(&mut system, &mut atlas, &second, mode);
            atlas.commit(frame);

            assert_eq!((a.dropped_glyphs, b.dropped_glyphs), (0, 0), "{mode:?}");
            assert!(!a.compacted && !b.compacted, "{mode:?}");
            let first_glyphs = atlas.keys_for(&first.glyphs, mode);
            assert!(
                first_glyphs
                    .iter()
                    .all(|key| atlas.glyphs.contains_key(key))
            );
        }
        assert!(atlas.size() <= 1024);
    }

    #[cfg(feature = "msdf")]
    mod msdf_tests {
        use super::*;
        use crate::msdf;

        const COLR_FONT: &[u8] = include_bytes!("../assets/roboto/RobotoColrTest.ttf");

        fn msdf_instances(prepared: &PreparedGlyphs) -> usize {
            prepared
                .instances
                .iter()
                .filter(|instance| instance.mode == GLYPH_MODE_MSDF)
                .count()
        }

        #[test]
        fn small_raster_fallbacks_share_the_msdf_baseline() {
            let mut system = TextSystem::new();
            let mut request = TextLayoutRequest::new("SMALL SIZES AT FRACTIONAL POSITIONS");
            request.font_size = 14.0;
            let shaped = system.shape(&request);
            let mut atlas = GlyphAtlas::new(1024);
            for y in [24.0, 24.6, -24.6] {
                let layout = shaped.place([40.3, y], [1.0; 4]);
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
                    let baseline = if instance.mode == GLYPH_MODE_MSDF {
                        instance.rect[1] + (placed.top - 0.5) * request.font_size / MSDF_EM_SIZE
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
                request.font_size = 32.0 + frame as f32 * (968.0 / 39.0);
                request.origin = [frame as f32 * 0.37, 0.0];
                let layout = system.layout(&request);
                fields.extend(
                    layout
                        .glyphs
                        .iter()
                        .map(|glyph| msdf::msdf_key(glyph.key.0)),
                );
                atlas.begin_frame();
                let prepared = prepare(&mut system, &mut atlas, &layout, TextRenderMode::Msdf);
                atlas.commit(prepared.frame);

                assert_eq!(prepared.dropped_glyphs, 0, "frame {frame}");
                assert!(msdf_instances(&prepared) > 0);
                uploads += prepared.uploads.iter().filter(|upload| {
                    atlas.glyphs.iter().any(|(key, glyph)| {
                        matches!(key, AtlasKey::Msdf(_)) && matches!(glyph.image, AtlasImage::Placed(placed) if placed.origin == upload.origin)
                    })
                }).count();
            }

            assert!(uploads <= fields.len());
            assert_eq!(atlas.size(), 4096);
        }

        #[test]
        fn colour_and_overlapping_glyphs_fall_back_to_raster() {
            let mut system = TextSystem::new();
            system.load_font_data(COLR_FONT.to_vec());
            let mut colour = TextLayoutRequest::new("A");
            colour.font_family = "Roboto COLR Test".to_string();
            colour.font_size = 48.0;
            let colour = system.layout(&colour);

            let mut overlapping = system.layout(&TextLayoutRequest::new("O"));
            let mut atlas = GlyphAtlas::new(512);
            atlas.begin_frame();
            let colour = prepare(&mut system, &mut atlas, &colour, TextRenderMode::Msdf);
            assert_eq!(colour.instances.len(), 1);
            assert_eq!(colour.instances[0].mode, GLYPH_MODE_RASTER_COLOR);

            // Two copies of one outline overlap everywhere.
            let key = msdf::msdf_key(overlapping.glyphs[0].key.0);
            let commands = system
                .swash_cache
                .get_outline_commands_uncached(&mut system.font_system, key)
                .unwrap();
            let overlaps = |commands: &[swash::zeno::Command]| {
                msdf::contours_overlap(&msdf::contours(commands, |p| {
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
}
