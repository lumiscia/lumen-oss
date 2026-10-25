//! Text layout and GPU-facing text data for Lumen.
//!
//! This crate deliberately owns text shaping, measurement, atlas bookkeeping,
//! and WGSL-facing buffer formats. Lumen nodes should depend on this layer
//! rather than coupling directly to a specific renderer such as glyphon.

use std::collections::{HashMap, HashSet};

use cosmic_text::{
    Align, Attrs, Buffer, CacheKey, CacheKeyFlags, Family, FontSystem, LayoutGlyph, Metrics,
    Shaping, Style, SwashCache, SwashContent, SwashImage, Weight, Wrap,
};

mod gpu;
#[cfg(feature = "experimental-msdf")]
mod msdf;
pub use gpu::*;
#[cfg(feature = "experimental-msdf")]
pub use msdf::*;

pub const ALPHA_TEXT_SHADER: &str = include_str!("shaders/alpha_text.wgsl");
#[cfg(feature = "experimental-msdf")]
pub const MSDF_TEXT_SHADER: &str = include_str!("shaders/msdf_text.wgsl");
#[cfg(feature = "experimental-msdf")]
pub const MSDF_GENERATOR_SHADER: &str = include_str!("shaders/msdf_generate.wgsl");
pub const DEFAULT_FONT_FAMILY: &str = "Roboto";
pub const ROBOTO_REGULAR_BYTES: &[u8] = include_bytes!("../assets/roboto/Roboto-Regular.ttf");

/// Cached glyph count (including empty and overflowed glyphs) above which a
/// raster atlas evicts glyphs unused in the current frame.
const MAX_RASTER_ATLAS_GLYPHS: usize = 16_384;

#[derive(Debug)]
pub struct TextSystem {
    font_system: FontSystem,
    swash_cache: SwashCache,
    font_generation: u64,
    #[cfg(feature = "experimental-msdf")]
    msdf_job_cache: HashMap<GlyphKey, Option<MsdfGlyphJob>>,
}

impl TextSystem {
    pub fn new() -> Self {
        let mut font_system = FontSystem::new_with_fonts(std::iter::empty());
        load_default_fonts(&mut font_system);
        Self {
            font_system,
            swash_cache: SwashCache::new(),
            font_generation: 0,
            #[cfg(feature = "experimental-msdf")]
            msdf_job_cache: HashMap::new(),
        }
    }

    pub fn load_font_data(&mut self, data: Vec<u8>) {
        self.font_system.db_mut().load_font_data(data);
        self.font_generation += 1;
        #[cfg(feature = "experimental-msdf")]
        self.msdf_job_cache.clear();
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
                    line_height: run.line_height,
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

    /// Rasterizes `layout` into a fresh atlas image.
    pub fn render_alpha_atlas(
        &mut self,
        layout: &TextLayout,
        config: AtlasConfig,
        max_glyphs: usize,
    ) -> AlphaAtlasRender {
        let mut atlas = RasterAtlas::new(config);
        let frame = self.prepare_raster_atlas(&mut atlas, layout, max_glyphs);
        let mut pixels = vec![0_u8; config.width as usize * config.height as usize * 4];
        for upload in &frame.uploads {
            copy_into_atlas(
                &mut pixels,
                config.width,
                upload.origin,
                upload.size,
                &upload.pixels,
            );
        }
        AlphaAtlasRender {
            pixels,
            glyph_count: frame.instances.len(),
            instances: frame.instances,
            dropped_glyphs: frame.dropped_glyphs,
        }
    }

    /// Places `layout` in `atlas`, rasterizing glyphs that are not resident yet.
    ///
    /// When the atlas is full and glyphs unused this frame hold at least half of
    /// it, the atlas is cleared and this frame's glyphs are packed again. Glyphs
    /// that still do not fit, and glyphs past `max_glyphs`, are dropped and
    /// counted. The returned uploads only become resident once
    /// [`RasterAtlas::commit`] is called with the returned frame.
    pub fn prepare_raster_atlas(
        &mut self,
        atlas: &mut RasterAtlas,
        layout: &TextLayout,
        max_glyphs: usize,
    ) -> RasterAtlasFrame {
        atlas.frame += 1;
        let frame = atlas.frame;
        let visible = &layout.glyphs[..layout.glyphs.len().min(max_glyphs)];
        let mut seen = HashSet::new();
        let keys: Vec<GlyphKey> = visible
            .iter()
            .map(|glyph| glyph.key)
            .filter(|key| seen.insert(*key))
            .collect();
        for key in &keys {
            if let Some(glyph) = atlas.glyphs.get_mut(key) {
                glyph.last_used = frame;
            }
        }

        let padding = atlas.config.padding();
        let mut missing: Vec<GlyphKey> = keys
            .iter()
            .copied()
            .filter(|key| !atlas.is_resident(key))
            .collect();
        let mut rasters: HashMap<GlyphKey, Option<RasterImage>> = missing
            .iter()
            .map(|&key| (key, self.rasterize(key, padding)))
            .collect();
        let compacted = atlas.has_stale()
            && (atlas.glyphs.len() > MAX_RASTER_ATLAS_GLYPHS
                || (!atlas.fits(&missing, &rasters) && atlas.mostly_stale()));
        if compacted {
            atlas.clear();
            for &key in &keys {
                rasters
                    .entry(key)
                    .or_insert_with(|| self.rasterize(key, padding));
            }
            missing = keys;
        }

        let mut uploads = Vec::new();
        for key in missing {
            let raster = rasters.remove(&key).flatten();
            atlas.insert(key, raster, &mut uploads);
        }

        let mut instances = Vec::with_capacity(visible.len());
        let mut dropped_glyphs = layout.glyphs.len() - visible.len();
        for glyph in visible {
            match atlas.glyphs.get(&glyph.key).map(|cached| cached.slot) {
                Some(RasterSlot::Placed { entry, placement }) => {
                    instances.push(glyph_instance_for(glyph, entry, placement));
                }
                Some(RasterSlot::Overflow) => dropped_glyphs += 1,
                Some(RasterSlot::Empty) | None => {}
            }
        }

        RasterAtlasFrame {
            frame,
            instances,
            uploads,
            dropped_glyphs,
            compacted,
        }
    }

    #[cfg(feature = "experimental-msdf")]
    pub fn render_gpu_hybrid_atlas(
        &mut self,
        layout: &TextLayout,
        config: AtlasConfig,
        max_glyphs: usize,
        max_segments: usize,
        max_msdf_pixels: u32,
    ) -> GpuHybridAtlasRender {
        let mut atlas = GlyphAtlas::new(config);
        let mut pixels = vec![0_u8; config.width as usize * config.height as usize * 4];
        let mut instances = Vec::with_capacity(max_glyphs.min(layout.glyphs.len()));
        let mut jobs = Vec::new();
        let mut segments = Vec::new();
        let mut pixel_jobs = Vec::new();
        let mut msdf_pixel_count = 0_u32;
        let mut glyph_count = 0;

        for glyph in layout.glyphs.iter().take(max_glyphs) {
            let msdf_key = glyph.key.msdf_key();
            if let Some(msdf) = self.glyph_msdf_job(msdf_key, config) {
                let glyph_size = [msdf.placement.width, msdf.placement.height];
                let glyph_pixels = glyph_size[0].saturating_mul(glyph_size[1]);
                if !msdf.segments.is_empty()
                    && segments.len().saturating_add(msdf.segments.len()) <= max_segments
                    && msdf_pixel_count.saturating_add(glyph_pixels) <= max_msdf_pixels
                {
                    let Some(entry) = atlas.ensure_glyph(msdf_key, glyph_size) else {
                        continue;
                    };
                    let segment_start = segments.len() as u32;
                    let pixel_start = msdf_pixel_count;
                    let job_index = jobs.len() as u32;
                    segments.extend(msdf.segments);
                    msdf_pixel_count = msdf_pixel_count.saturating_add(glyph_pixels);
                    pixel_jobs.extend(std::iter::repeat(job_index).take(glyph_pixels as usize));
                    jobs.push(GpuMsdfJob {
                        atlas_rect: [
                            entry.origin[0],
                            entry.origin[1],
                            entry.size[0],
                            entry.size[1],
                        ],
                        segment_range: [segment_start, segments.len() as u32 - segment_start],
                        pixel_range: [pixel_start, glyph_pixels],
                        px_range: config.px_range.max(1) as f32,
                        _padding: [0; 3],
                    });
                    instances.push(msdf_glyph_instance_for(glyph, entry, &msdf.placement));
                } else {
                    let Some(instance) =
                        self.raster_glyph_instance(glyph, &mut atlas, config, &mut pixels)
                    else {
                        continue;
                    };
                    instances.push(instance);
                }
            } else {
                let Some(instance) =
                    self.raster_glyph_instance(glyph, &mut atlas, config, &mut pixels)
                else {
                    continue;
                };
                instances.push(instance);
            }
            glyph_count += 1;
        }

        GpuHybridAtlasRender {
            atlas,
            pixels,
            instances,
            jobs,
            segments,
            pixel_jobs,
            msdf_pixel_count,
            glyph_count,
        }
    }

    /// Rasterizes without Swash's image cache; the atlas is the only glyph cache.
    fn rasterize(&mut self, key: GlyphKey, padding: u32) -> Option<RasterImage> {
        let image = self
            .swash_cache
            .get_image_uncached(&mut self.font_system, key.0)?;
        Some(RasterImage::from_swash(&image, padding))
    }

    #[cfg(feature = "experimental-msdf")]
    fn glyph_msdf_job(&mut self, key: GlyphKey, config: AtlasConfig) -> Option<MsdfGlyphJob> {
        if let Some(cached) = self.msdf_job_cache.get(&key) {
            return cached.clone();
        }
        self.font_system
            .db()
            .with_face_data(key.0.font_id, |data, face_index| {
                generate_msdf_job(data, face_index, key.0, config)
            })
            .flatten()
            .inspect(|job| {
                self.msdf_job_cache.insert(key, Some(job.clone()));
            })
            .or_else(|| {
                self.msdf_job_cache.insert(key, None);
                None
            })
    }

    #[cfg(feature = "experimental-msdf")]
    fn raster_glyph_instance(
        &mut self,
        glyph: &TextGlyph,
        atlas: &mut GlyphAtlas,
        config: AtlasConfig,
        pixels: &mut [u8],
    ) -> Option<GpuGlyphInstance> {
        let image = self.rasterize(glyph.key, config.padding())?;
        let glyph_size = [image.placement.width, image.placement.height];
        let entry = atlas.ensure_glyph(glyph.key, glyph_size)?;
        copy_into_atlas(
            pixels,
            config.width,
            image.slot_origin(entry),
            image.padded_size(),
            &image.pixels,
        );
        Some(glyph_instance_for(glyph, entry, image.placement))
    }
}

impl Default for TextSystem {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "experimental-msdf")]
pub fn rgba8_to_rgba16_float(pixels: &[u8]) -> Vec<u16> {
    pixels
        .iter()
        .map(|value| half::f16::from_f32(f32::from(*value) / 255.0).to_bits())
        .collect()
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
    line_height: f32,
}

impl ShapedText {
    /// Places glyphs at `origin`. The fractional origin selects each glyph's
    /// subpixel bin, and glyph positions are snapped to whole pixels.
    pub fn place(&self, origin: [f32; 2], color: [f32; 4]) -> TextLayout {
        let glyphs = self
            .glyphs
            .iter()
            .map(|shaped| {
                let glyph = &shaped.glyph;
                let physical = glyph.physical((origin[0], origin[1] + shaped.line_y), 1.0);
                TextGlyph {
                    key: GlyphKey(physical.cache_key),
                    x: physical.x as f32,
                    y: physical.y as f32,
                    width: glyph.w,
                    height: shaped.line_height,
                    x_offset: glyph.x_offset,
                    y_offset: glyph.y_offset,
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
    pub width: f32,
    pub height: f32,
    pub x_offset: f32,
    pub y_offset: f32,
    pub color: [f32; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct GlyphKey(pub CacheKey);

impl GlyphKey {
    #[cfg(feature = "experimental-msdf")]
    fn msdf_key(self) -> Self {
        let mut key = self.0;
        key.x_bin = cosmic_text::SubpixelBin::Zero;
        key.y_bin = cosmic_text::SubpixelBin::Zero;
        Self(key)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtlasConfig {
    pub width: u32,
    pub height: u32,
    pub px_range: u32,
}

impl AtlasConfig {
    /// Transparent border kept around each glyph slot.
    pub fn padding(&self) -> u32 {
        self.px_range.max(1)
    }
}

impl Default for AtlasConfig {
    fn default() -> Self {
        Self {
            width: 2048,
            height: 2048,
            px_range: 8,
        }
    }
}

#[derive(Debug, Clone)]
struct ShelfPacker {
    width: u32,
    height: u32,
    cursor_x: u32,
    cursor_y: u32,
    row_height: u32,
}

impl ShelfPacker {
    fn new(config: AtlasConfig) -> Self {
        Self {
            width: config.width,
            height: config.height,
            cursor_x: 0,
            cursor_y: 0,
            row_height: 0,
        }
    }

    fn clear(&mut self) {
        self.cursor_x = 0;
        self.cursor_y = 0;
        self.row_height = 0;
    }

    /// Reserves a rectangle and returns its top-left corner.
    fn allocate(&mut self, size: [u32; 2]) -> Option<[u32; 2]> {
        let width = size[0].max(1);
        let height = size[1].max(1);
        if width > self.width || height > self.height {
            return None;
        }
        let (x, y, row_height) = if self.cursor_x + width > self.width {
            (0, self.cursor_y.saturating_add(self.row_height), 0)
        } else {
            (self.cursor_x, self.cursor_y, self.row_height)
        };
        if y.saturating_add(height) > self.height {
            return None;
        }
        self.cursor_x = x + width;
        self.cursor_y = y;
        self.row_height = row_height.max(height);
        Some([x, y])
    }

    fn used_height(&self) -> u32 {
        self.cursor_y
            .saturating_add(self.row_height)
            .min(self.height)
    }
}

/// Single-use glyph atlas that never evicts.
#[derive(Debug, Clone)]
pub struct GlyphAtlas {
    config: AtlasConfig,
    entries: HashMap<GlyphKey, AtlasEntry>,
    packer: ShelfPacker,
}

impl GlyphAtlas {
    pub fn new(config: AtlasConfig) -> Self {
        Self {
            config,
            entries: HashMap::new(),
            packer: ShelfPacker::new(config),
        }
    }

    pub fn config(&self) -> AtlasConfig {
        self.config
    }

    pub fn used_size(&self) -> [u32; 2] {
        [self.config.width, self.packer.used_height()]
    }

    pub fn entry(&self, key: &GlyphKey) -> Option<AtlasEntry> {
        self.entries.get(key).copied()
    }

    pub fn ensure_glyph(&mut self, key: GlyphKey, size: [u32; 2]) -> Option<AtlasEntry> {
        if let Some(entry) = self.entry(&key) {
            return Some(entry);
        }
        let padding = self.config.padding();
        let slot = self.packer.allocate([
            size[0].saturating_add(padding * 2),
            size[1].saturating_add(padding * 2),
        ])?;
        let entry = AtlasEntry::new(self.config, slot, padding, size);
        self.entries.insert(key, entry);
        Some(entry)
    }
}

/// Glyph atlas that persists across frames. Only placement is kept on the CPU;
/// glyph pixels exist solely in the uploads returned for the frame.
#[derive(Debug, Clone)]
pub struct RasterAtlas {
    config: AtlasConfig,
    packer: ShelfPacker,
    glyphs: HashMap<GlyphKey, RasterGlyph>,
    frame: u64,
}

#[derive(Debug, Clone, Copy)]
struct RasterGlyph {
    slot: RasterSlot,
    last_used: u64,
    upload_frame: u64,
    resident: bool,
}

#[derive(Debug, Clone, Copy)]
enum RasterSlot {
    /// Nothing to draw, such as whitespace.
    Empty,
    /// Did not fit in the atlas.
    Overflow,
    Placed {
        entry: AtlasEntry,
        placement: GlyphPlacement,
    },
}

impl RasterAtlas {
    pub fn new(config: AtlasConfig) -> Self {
        Self {
            config,
            packer: ShelfPacker::new(config),
            glyphs: HashMap::new(),
            frame: 0,
        }
    }

    pub fn config(&self) -> AtlasConfig {
        self.config
    }

    /// Number of cached glyphs, including empty and overflowed ones.
    pub fn len(&self) -> usize {
        self.glyphs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }

    /// Marks the uploads of `frame` as written to the GPU texture.
    pub fn commit(&mut self, frame: u64) {
        for glyph in self.glyphs.values_mut() {
            if glyph.upload_frame == frame {
                glyph.resident = true;
            }
        }
    }

    fn clear(&mut self) {
        self.glyphs.clear();
        self.packer.clear();
    }

    fn is_resident(&self, key: &GlyphKey) -> bool {
        self.glyphs.get(key).is_some_and(|glyph| glyph.resident)
    }

    fn has_stale(&self) -> bool {
        self.glyphs
            .values()
            .any(|glyph| glyph.last_used != self.frame)
    }

    /// Whether glyphs unused this frame hold at least half of the allocated area.
    fn mostly_stale(&self) -> bool {
        let padding = u64::from(self.config.padding());
        let mut stale = 0_u64;
        let mut total = 0_u64;
        for glyph in self.glyphs.values() {
            let RasterSlot::Placed { entry, .. } = glyph.slot else {
                continue;
            };
            let area =
                (u64::from(entry.size[0]) + padding * 2) * (u64::from(entry.size[1]) + padding * 2);
            total += area;
            if glyph.last_used != self.frame {
                stale += area;
            }
        }
        stale > 0 && stale * 2 >= total
    }

    fn fits(&self, keys: &[GlyphKey], rasters: &HashMap<GlyphKey, Option<RasterImage>>) -> bool {
        let mut packer = self.packer.clone();
        keys.iter().all(|key| {
            if let Some(RasterGlyph {
                slot: RasterSlot::Placed { .. },
                ..
            }) = self.glyphs.get(key)
            {
                return true;
            }
            match rasters.get(key).and_then(Option::as_ref) {
                Some(raster) if !raster.is_empty() => {
                    packer.allocate(raster.padded_size()).is_some()
                }
                _ => true,
            }
        })
    }

    fn insert(
        &mut self,
        key: GlyphKey,
        raster: Option<RasterImage>,
        uploads: &mut Vec<GlyphUpload>,
    ) {
        let existing = self.glyphs.get(&key).map(|glyph| glyph.slot);
        let slot = match raster {
            Some(raster) if !raster.is_empty() => {
                let size = [raster.placement.width, raster.placement.height];
                let entry = match existing {
                    Some(RasterSlot::Placed { entry, .. }) => Some(entry),
                    _ => self
                        .packer
                        .allocate(raster.padded_size())
                        .map(|slot| AtlasEntry::new(self.config, slot, raster.padding, size)),
                };
                match entry {
                    Some(entry) => {
                        uploads.push(GlyphUpload {
                            origin: raster.slot_origin(entry),
                            size: raster.padded_size(),
                            pixels: raster.pixels,
                        });
                        RasterSlot::Placed {
                            entry,
                            placement: raster.placement,
                        }
                    }
                    None => RasterSlot::Overflow,
                }
            }
            _ => RasterSlot::Empty,
        };
        self.glyphs.insert(
            key,
            RasterGlyph {
                slot,
                last_used: self.frame,
                upload_frame: self.frame,
                resident: !matches!(slot, RasterSlot::Placed { .. }),
            },
        );
    }
}

/// Glyph image placement relative to the pen position, in whole pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlyphPlacement {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub color: bool,
}

#[derive(Debug, Clone)]
struct RasterImage {
    placement: GlyphPlacement,
    padding: u32,
    /// Premultiplied RGBA8 rows, including a transparent `padding` border.
    pixels: Vec<u8>,
}

impl RasterImage {
    fn from_swash(image: &SwashImage, padding: u32) -> Self {
        let width = image.placement.width as usize;
        let height = image.placement.height as usize;
        let pad = padding as usize;
        let padded_width = width + pad * 2;
        let mut pixels = vec![0_u8; padded_width * (height + pad * 2) * 4];
        // Swash composites colour outlines premultiplied but decodes colour
        // bitmaps with straight alpha.
        let straight_color = matches!(image.source, swash::scale::Source::ColorBitmap(_));
        for y in 0..height {
            for x in 0..width {
                let src = y * width + x;
                let rgba = match image.content {
                    SwashContent::Mask => {
                        let alpha = image.data[src];
                        [alpha; 4]
                    }
                    SwashContent::SubpixelMask => {
                        let rgb = &image.data[src * 4..src * 4 + 3];
                        let alpha = rgb[0].max(rgb[1]).max(rgb[2]);
                        [alpha; 4]
                    }
                    SwashContent::Color => {
                        let [r, g, b, a] = image.data[src * 4..src * 4 + 4] else {
                            unreachable!()
                        };
                        if straight_color {
                            [premultiply(r, a), premultiply(g, a), premultiply(b, a), a]
                        } else {
                            [r, g, b, a]
                        }
                    }
                };
                let dst = ((y + pad) * padded_width + x + pad) * 4;
                pixels[dst..dst + 4].copy_from_slice(&rgba);
            }
        }
        Self {
            placement: GlyphPlacement {
                left: image.placement.left,
                top: image.placement.top,
                width: image.placement.width,
                height: image.placement.height,
                color: image.content == SwashContent::Color,
            },
            padding,
            pixels,
        }
    }

    fn is_empty(&self) -> bool {
        self.placement.width == 0 || self.placement.height == 0
    }

    fn padded_size(&self) -> [u32; 2] {
        [
            self.placement.width + self.padding * 2,
            self.placement.height + self.padding * 2,
        ]
    }

    fn slot_origin(&self, entry: AtlasEntry) -> [u32; 2] {
        [
            entry.origin[0] - self.padding,
            entry.origin[1] - self.padding,
        ]
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
    /// Tightly packed premultiplied RGBA8 rows.
    pub pixels: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct RasterAtlasFrame {
    /// Pass to [`RasterAtlas::commit`] once `uploads` have been written.
    pub frame: u64,
    pub instances: Vec<GpuGlyphInstance>,
    pub uploads: Vec<GlyphUpload>,
    /// Glyphs not drawn because of `max_glyphs` or atlas capacity.
    pub dropped_glyphs: usize,
    /// The atlas was cleared and repacked this frame.
    pub compacted: bool,
}

#[derive(Debug, Clone)]
pub struct AlphaAtlasRender {
    pub pixels: Vec<u8>,
    pub instances: Vec<GpuGlyphInstance>,
    pub glyph_count: usize,
    pub dropped_glyphs: usize,
}

#[cfg(feature = "experimental-msdf")]
#[derive(Debug, Clone)]
pub struct GpuHybridAtlasRender {
    pub atlas: GlyphAtlas,
    pub pixels: Vec<u8>,
    pub instances: Vec<GpuGlyphInstance>,
    pub jobs: Vec<GpuMsdfJob>,
    pub segments: Vec<GpuMsdfSegment>,
    pub pixel_jobs: Vec<u32>,
    pub msdf_pixel_count: u32,
    pub glyph_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AtlasEntry {
    pub origin: [u32; 2],
    pub size: [u32; 2],
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
}

impl AtlasEntry {
    fn new(config: AtlasConfig, slot: [u32; 2], padding: u32, size: [u32; 2]) -> Self {
        let origin = [slot[0] + padding, slot[1] + padding];
        let atlas_size = [config.width as f32, config.height as f32];
        Self {
            origin,
            size,
            uv_min: [
                origin[0] as f32 / atlas_size[0],
                origin[1] as f32 / atlas_size[1],
            ],
            uv_max: [
                (origin[0] + size[0]) as f32 / atlas_size[0],
                (origin[1] + size[1]) as f32 / atlas_size[1],
            ],
        }
    }
}

fn copy_into_atlas(
    pixels: &mut [u8],
    atlas_width: u32,
    origin: [u32; 2],
    size: [u32; 2],
    data: &[u8],
) {
    let row_len = size[0] as usize * 4;
    for row in 0..size[1] as usize {
        let src = row * row_len;
        let dst = ((origin[1] as usize + row) * atlas_width as usize + origin[0] as usize) * 4;
        pixels[dst..dst + row_len].copy_from_slice(&data[src..src + row_len]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn creates_gpu_glyph_instances_from_layout() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("Roboto");
        request.font_size = 32.0;
        let layout = system.layout(&request);
        let render = system.render_alpha_atlas(&layout, AtlasConfig::default(), 128);
        let instance = render
            .instances
            .iter()
            .find(|instance| instance.rect[2] > 0.0)
            .expect("expected a visible glyph instance");

        assert!(instance.rect[2] > 0.0);
        assert!(instance.uv_rect[2] > instance.uv_rect[0]);
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
    fn alpha_atlas_contains_non_empty_pixels() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("A");
        request.font_size = 48.0;
        let layout = system.layout(&request);
        let render = system.render_alpha_atlas(
            &layout,
            AtlasConfig {
                width: 128,
                height: 128,
                px_range: 2,
            },
            16,
        );

        assert_eq!(render.instances.len(), render.glyph_count);
        assert!(render.glyph_count > 0);
        assert!(render.pixels.chunks_exact(4).any(|pixel| pixel[3] > 0));
    }

    #[test]
    fn colour_glyphs_are_stored_premultiplied() {
        let mut image = SwashImage::new();
        image.content = SwashContent::Color;
        image.placement.width = 2;
        image.placement.height = 1;
        image.data = vec![255, 0, 0, 128, 0, 100, 200, 255];

        image.source = swash::scale::Source::ColorBitmap(swash::scale::StrikeWith::BestFit);
        let bitmap = RasterImage::from_swash(&image, 1);
        image.source = swash::scale::Source::ColorOutline(0);
        let outline = RasterImage::from_swash(&image, 1);

        // Row 1 of the 4x3 padded image starts at pixel 4; the glyph starts at pixel 5.
        let first = 5 * 4;
        assert_eq!(
            &bitmap.pixels[first..first + 8],
            &[128, 0, 0, 128, 0, 100, 200, 255]
        );
        assert_eq!(&outline.pixels[first..first + 8], image.data.as_slice());
        assert!(bitmap.placement.color);
        assert!(bitmap.pixels[..first].iter().all(|value| *value == 0));
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
        let render = system.render_alpha_atlas(&second, AtlasConfig::default(), 128);

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
    fn font_size_sweep_keeps_raster_memory_bounded() {
        let mut system = TextSystem::new();
        let mut atlas = RasterAtlas::new(AtlasConfig {
            width: 2048,
            height: 2048,
            px_range: 1,
        });
        let mut seen = HashSet::new();
        let mut compactions = 0;
        for frame in 0..60 {
            let mut request = TextLayoutRequest::new("Lumen text");
            request.font_size = 32.0 + frame as f32 * (568.0 / 59.0);
            let layout = system.layout(&request);
            seen.extend(layout.glyphs.iter().map(|glyph| glyph.key));
            let prepared = system.prepare_raster_atlas(&mut atlas, &layout, 128);
            atlas.commit(prepared.frame);

            assert_eq!(prepared.dropped_glyphs, 0, "frame {frame}");
            compactions += usize::from(prepared.compacted);
        }

        assert!(compactions > 0);
        assert!(atlas.len() < seen.len() / 2);
        assert!(system.swash_cache.image_cache.is_empty());
    }

    #[test]
    fn atlas_overflow_drops_glyphs_without_thrashing() {
        let mut system = TextSystem::new();
        let mut atlas = RasterAtlas::new(AtlasConfig {
            width: 256,
            height: 256,
            px_range: 1,
        });
        let mut request = TextLayoutRequest::new("ABCDEFGHIJKLMNOPQRSTUVWXYZ");
        request.font_size = 120.0;
        let layout = system.layout(&request);

        let first = system.prepare_raster_atlas(&mut atlas, &layout, 128);
        atlas.commit(first.frame);
        let second = system.prepare_raster_atlas(&mut atlas, &layout, 128);

        assert!(!first.instances.is_empty());
        assert!(first.dropped_glyphs > 0);
        assert_eq!(
            first.instances.len() + first.dropped_glyphs,
            layout.glyphs.len()
        );
        assert!(second.uploads.is_empty());
        assert!(!second.compacted);
        assert_eq!(second.dropped_glyphs, first.dropped_glyphs);
        assert_eq!(second.instances.len(), first.instances.len());

        let capped = system.prepare_raster_atlas(&mut atlas, &layout, 4);
        assert_eq!(capped.dropped_glyphs, layout.glyphs.len() - 4);
    }

    #[test]
    fn uncommitted_uploads_are_repeated() {
        let mut system = TextSystem::new();
        let mut atlas = RasterAtlas::new(AtlasConfig::default());
        let layout = system.layout(&TextLayoutRequest::new("Retry"));

        let dropped = system.prepare_raster_atlas(&mut atlas, &layout, 128);
        let retried = system.prepare_raster_atlas(&mut atlas, &layout, 128);
        atlas.commit(retried.frame);
        let committed = system.prepare_raster_atlas(&mut atlas, &layout, 128);

        assert!(!dropped.uploads.is_empty());
        assert_eq!(retried.uploads.len(), dropped.uploads.len());
        assert_eq!(retried.uploads[0].origin, dropped.uploads[0].origin);
        assert!(committed.uploads.is_empty());
    }

    #[cfg(feature = "experimental-msdf")]
    #[test]
    fn hybrid_atlas_uses_msdf_for_outline_glyphs() {
        let mut system = TextSystem::new();
        let layout = system.layout(&TextLayoutRequest::new("A"));
        let render =
            system.render_gpu_hybrid_atlas(&layout, AtlasConfig::default(), 16, 4096, 4096);

        assert!(render.instances.iter().any(is_msdf_instance));
    }

    #[cfg(feature = "experimental-msdf")]
    #[test]
    fn msdf_keys_ignore_subpixel_position() {
        let mut system = TextSystem::new();
        let mut first = TextLayoutRequest::new("A");
        first.origin = [0.1, 0.0];
        let mut second = first.clone();
        second.origin = [0.6, 0.0];

        let first_key = system.layout(&first).glyphs[0].key;
        let second_key = system.layout(&second).glyphs[0].key;

        assert_ne!(first_key, second_key);
        assert_eq!(first_key.msdf_key(), second_key.msdf_key());
    }

    #[cfg(feature = "experimental-msdf")]
    #[test]
    fn msdf_placement_keeps_fractional_bearings() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("B");
        request.font_size = 48.3;
        let layout = system.layout(&request);
        let job = system
            .glyph_msdf_job(layout.glyphs[0].key.msdf_key(), AtlasConfig::default())
            .expect("expected outline glyph to produce an MSDF job");

        assert_ne!(job.placement.top.fract(), 0.0);
    }

    #[cfg(feature = "experimental-msdf")]
    #[test]
    fn msdf_budget_fallback_uses_raster_atlas_entry() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("A");
        request.origin = [0.25, 0.0];
        let layout = system.layout(&request);
        let glyph_key = layout.glyphs[0].key;

        let render =
            system.render_gpu_hybrid_atlas(&layout, AtlasConfig::default(), 16, 0, u32::MAX);

        assert!(render.jobs.is_empty());
        assert!(
            render
                .instances
                .iter()
                .all(|instance| !is_msdf_instance(instance))
        );
        assert!(render.atlas.entry(&glyph_key).is_some());
    }

    #[test]
    fn atlas_reuses_existing_glyph_entries() {
        let key = {
            let mut system = TextSystem::new();
            let layout = system.layout(&TextLayoutRequest::new("A"));
            layout.glyphs[0].key
        };
        let mut atlas = GlyphAtlas::new(AtlasConfig {
            width: 64,
            height: 64,
            px_range: 2,
        });

        let first = atlas.ensure_glyph(key, [12, 16]).unwrap();
        let second = atlas.ensure_glyph(key, [12, 16]).unwrap();

        assert_eq!(first, second);
    }

    #[test]
    fn gpu_structs_have_stable_sizes() {
        assert_eq!(std::mem::size_of::<GpuTextGlobals>(), 32);
        assert_eq!(std::mem::size_of::<GpuGlyphInstance>(), 64);
        #[cfg(feature = "experimental-msdf")]
        {
            assert_eq!(std::mem::size_of::<GpuMsdfGlobals>(), 24);
            assert_eq!(std::mem::size_of::<GpuMsdfJob>(), 48);
            assert_eq!(std::mem::size_of::<GpuMsdfSegment>(), 48);
        }
    }

    #[test]
    fn alpha_text_shader_is_valid_wgsl() {
        let Ok(renderer) = pollster::block_on(lumen_gpu::Renderer::new()) else {
            return;
        };
        let _ = renderer
            .device
            .create_shader_module(lumen_gpu::wgpu::ShaderModuleDescriptor {
                label: Some("alpha text shader validation"),
                source: lumen_gpu::wgpu::ShaderSource::Wgsl(ALPHA_TEXT_SHADER.into()),
            });
    }

    #[cfg(feature = "experimental-msdf")]
    #[test]
    fn msdf_shaders_are_valid_wgsl() {
        let Ok(renderer) = pollster::block_on(lumen_gpu::Renderer::new()) else {
            return;
        };
        for (label, shader) in [
            ("msdf text shader validation", MSDF_TEXT_SHADER),
            ("msdf generator shader validation", MSDF_GENERATOR_SHADER),
        ] {
            let _ = renderer
                .device
                .create_shader_module(lumen_gpu::wgpu::ShaderModuleDescriptor {
                    label: Some(label),
                    source: lumen_gpu::wgpu::ShaderSource::Wgsl(shader.into()),
                });
        }
    }

    #[cfg(feature = "experimental-msdf")]
    #[test]
    fn exposes_experimental_msdf_shader_sources() {
        #[cfg(feature = "experimental-msdf")]
        {
            assert!(!MSDF_TEXT_SHADER.is_empty());
            assert!(!MSDF_GENERATOR_SHADER.is_empty());
        }
    }
}
