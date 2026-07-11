//! Text layout and GPU-facing text data for Lumen.
//!
//! This crate deliberately owns text shaping, measurement, atlas bookkeeping,
//! and WGSL-facing buffer formats. Lumen nodes should depend on this layer
//! rather than coupling directly to a specific renderer such as glyphon.

use std::collections::HashMap;

use cosmic_text::{
    Align, Attrs, Buffer, CacheKey, CacheKeyFlags, Family, FontSystem, Metrics, Shaping, Style,
    SwashCache, SwashContent, SwashImage, Weight, Wrap,
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

#[cfg(feature = "experimental-msdf")]
const MSDF_JOB_CACHE_MAX_ENTRIES: usize = 256;
#[cfg(feature = "experimental-msdf")]
const MSDF_JOB_CACHE_MAX_SEGMENTS: usize = 65_536;

#[derive(Debug)]
pub struct TextSystem {
    font_system: FontSystem,
    swash_cache: SwashCache,
    #[cfg(feature = "experimental-msdf")]
    msdf_job_cache: MsdfJobCache,
}

impl TextSystem {
    pub fn new() -> Self {
        let mut font_system = FontSystem::new_with_fonts(std::iter::empty());
        load_default_fonts(&mut font_system);
        Self {
            font_system,
            swash_cache: SwashCache::new(),
            #[cfg(feature = "experimental-msdf")]
            msdf_job_cache: MsdfJobCache::default(),
        }
    }

    pub fn load_font_data(&mut self, data: Vec<u8>) {
        self.font_system.db_mut().load_font_data(data);
        #[cfg(feature = "experimental-msdf")]
        self.msdf_job_cache.clear();
    }

    pub fn measure(&mut self, request: &TextLayoutRequest) -> TextMeasurement {
        self.layout(request).measurement
    }

    pub fn layout(&mut self, request: &TextLayoutRequest) -> TextLayout {
        let metrics = Metrics::new(request.font_size.max(1.0), request.line_height());
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_size(
            &mut self.font_system,
            request.max_width.filter(|width| *width > 0.0),
            None,
        );
        buffer.set_wrap(
            &mut self.font_system,
            if request.max_width.is_some_and(|width| width > 0.0) {
                Wrap::WordOrGlyph
            } else {
                Wrap::None
            },
        );
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
            &mut self.font_system,
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
        for run in buffer.layout_runs() {
            width = width.max(run.line_w);
            height = height.max(run.line_top + run.line_height);
            for glyph in run.glyphs {
                let physical =
                    glyph.physical((request.origin[0], request.origin[1] + run.line_y), 1.0);
                glyphs.push(TextGlyph {
                    // Swash rasterizes the fractional position encoded in the cache key.
                    // Drawing that mask at Cosmic Text's integer physical position avoids
                    // linearly filtering an already-antialiased glyph a second time.
                    key: GlyphKey(physical.cache_key),
                    x: physical.x as f32,
                    y: physical.y as f32,
                    width: glyph.w,
                    height: run.line_height,
                    x_offset: glyph.x_offset,
                    y_offset: glyph.y_offset,
                    color: request.color,
                });
            }
        }

        TextLayout {
            measurement: TextMeasurement { width, height },
            glyphs,
        }
    }

    pub fn render_alpha_atlas(
        &mut self,
        layout: &TextLayout,
        config: AtlasConfig,
        max_glyphs: usize,
    ) -> AlphaAtlasRender {
        self.render_atlas(layout, config, max_glyphs)
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
        let mut msdf_pixel_count = 0_u32;
        let mut glyph_count = 0;
        let mut prepared_msdf = HashMap::<GlyphKey, PreparedHybridGlyph>::new();
        let mut prepared_raster = HashMap::<GlyphKey, PreparedHybridGlyph>::new();

        for glyph in layout.glyphs.iter().take(max_glyphs) {
            let msdf_key = glyph.key.msdf_key();
            if let Some(prepared) = prepared_msdf.get(&msdf_key) {
                instances.push(prepared.instance_for(glyph));
                glyph_count += 1;
                continue;
            }
            if let Some(prepared) = prepared_raster.get(&glyph.key) {
                instances.push(prepared.instance_for(glyph));
                glyph_count += 1;
                continue;
            }
            if let Some(msdf) = self.glyph_msdf_job(msdf_key, config) {
                let glyph_size = [msdf.placement.width, msdf.placement.height];
                let glyph_pixels = glyph_size[0].saturating_mul(glyph_size[1]);
                if !msdf.segments.is_empty()
                    && segments.len().saturating_add(msdf.segments.len()) <= max_segments
                    && msdf_pixel_count.saturating_add(glyph_pixels) <= max_msdf_pixels
                {
                    // The generated field already contains `px_range` pixels around the
                    // outline. Only reserve a texel for filtering isolation here; using the
                    // normal atlas padding would charge the range twice for every large glyph.
                    let Some(entry) = atlas.ensure_glyph_with_padding(msdf_key, glyph_size, 1)
                    else {
                        continue;
                    };
                    let segment_start = segments.len() as u32;
                    let pixel_start = msdf_pixel_count;
                    segments.extend(msdf.segments);
                    msdf_pixel_count = msdf_pixel_count.saturating_add(glyph_pixels);
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
                    let prepared_glyph = PreparedHybridGlyph::Msdf {
                        entry,
                        placement: msdf.placement,
                    };
                    instances.push(prepared_glyph.instance_for(glyph));
                    prepared_msdf.insert(msdf_key, prepared_glyph);
                } else {
                    let Some(prepared_glyph) =
                        self.raster_glyph(glyph, &mut atlas, config, &mut pixels)
                    else {
                        continue;
                    };
                    instances.push(prepared_glyph.instance_for(glyph));
                    prepared_raster.insert(glyph.key, prepared_glyph);
                }
            } else {
                let Some(prepared_glyph) =
                    self.raster_glyph(glyph, &mut atlas, config, &mut pixels)
                else {
                    continue;
                };
                instances.push(prepared_glyph.instance_for(glyph));
                prepared_raster.insert(glyph.key, prepared_glyph);
            }
            glyph_count += 1;
        }

        GpuHybridAtlasRender {
            atlas,
            pixels,
            instances,
            jobs,
            segments,
            msdf_pixel_count,
            glyph_count,
        }
    }

    fn render_atlas(
        &mut self,
        layout: &TextLayout,
        config: AtlasConfig,
        max_glyphs: usize,
    ) -> AlphaAtlasRender {
        let mut atlas = GlyphAtlas::new(config);
        let mut pixels = vec![0_u8; config.width as usize * config.height as usize * 4];
        let mut instances = Vec::with_capacity(max_glyphs.min(layout.glyphs.len()));
        let mut glyph_count = 0;

        for glyph in layout.glyphs.iter().take(max_glyphs) {
            let Some(image) = self.glyph_image(glyph.key) else {
                continue;
            };
            let glyph_size = [image.placement.width, image.placement.height];
            let Some(entry) = atlas.ensure_glyph(glyph.key, glyph_size) else {
                continue;
            };
            write_glyph_to_atlas(&mut pixels, config, entry, &image);
            instances.push(glyph_instance_for(glyph, entry, &image));
            glyph_count += 1;
        }

        AlphaAtlasRender {
            atlas,
            pixels,
            instances,
            glyph_count,
        }
    }

    fn glyph_image(&mut self, key: GlyphKey) -> Option<SwashImage> {
        self.swash_cache
            .get_image(&mut self.font_system, key.0)
            .as_ref()
            .cloned()
    }

    #[cfg(feature = "experimental-msdf")]
    fn glyph_msdf_job(&mut self, key: GlyphKey, config: AtlasConfig) -> Option<MsdfGlyphJob> {
        let cache_key = MsdfJobCacheKey { glyph: key, config };
        if let Some(cached) = self.msdf_job_cache.get(cache_key) {
            return cached;
        }
        let generated = self
            .font_system
            .db()
            .with_face_data(key.0.font_id, |data, face_index| {
                generate_msdf_job(data, face_index, key.0, config)
            })
            .flatten();
        self.msdf_job_cache.insert(cache_key, generated.clone());
        generated
    }

    #[cfg(feature = "experimental-msdf")]
    fn raster_glyph(
        &mut self,
        glyph: &TextGlyph,
        atlas: &mut GlyphAtlas,
        config: AtlasConfig,
        pixels: &mut [u8],
    ) -> Option<PreparedHybridGlyph> {
        let image = self.glyph_image(glyph.key)?;
        let glyph_size = [image.placement.width, image.placement.height];
        let entry = atlas.ensure_glyph(glyph.key, glyph_size)?;
        write_glyph_to_atlas(pixels, config, entry, &image);
        Some(PreparedHybridGlyph::Raster { entry, image })
    }
}

#[cfg(feature = "experimental-msdf")]
#[derive(Clone)]
enum PreparedHybridGlyph {
    Msdf {
        entry: AtlasEntry,
        placement: MsdfGlyphPlacement,
    },
    Raster {
        entry: AtlasEntry,
        image: SwashImage,
    },
}

#[cfg(feature = "experimental-msdf")]
impl PreparedHybridGlyph {
    fn instance_for(&self, glyph: &TextGlyph) -> GpuGlyphInstance {
        match self {
            Self::Msdf { entry, placement } => msdf_glyph_instance_for(glyph, *entry, placement),
            Self::Raster { entry, image } => glyph_instance_for(glyph, *entry, image),
        }
    }
}

#[cfg(feature = "experimental-msdf")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct MsdfJobCacheKey {
    glyph: GlyphKey,
    config: AtlasConfig,
}

#[cfg(feature = "experimental-msdf")]
#[derive(Debug, Clone)]
struct CachedMsdfJob {
    job: Option<MsdfGlyphJob>,
    last_used: u64,
    segment_count: usize,
}

#[cfg(feature = "experimental-msdf")]
#[derive(Debug, Default)]
struct MsdfJobCache {
    entries: HashMap<MsdfJobCacheKey, CachedMsdfJob>,
    clock: u64,
    segment_count: usize,
}

#[cfg(feature = "experimental-msdf")]
impl MsdfJobCache {
    fn get(&mut self, key: MsdfJobCacheKey) -> Option<Option<MsdfGlyphJob>> {
        let entry = self.entries.get_mut(&key)?;
        self.clock = self.clock.wrapping_add(1);
        entry.last_used = self.clock;
        Some(entry.job.clone())
    }

    fn insert(&mut self, key: MsdfJobCacheKey, job: Option<MsdfGlyphJob>) {
        self.clock = self.clock.wrapping_add(1);
        let segment_count = job.as_ref().map_or(0, |job| job.segments.len());
        if segment_count > MSDF_JOB_CACHE_MAX_SEGMENTS {
            return;
        }
        if let Some(replaced) = self.entries.insert(
            key,
            CachedMsdfJob {
                job,
                last_used: self.clock,
                segment_count,
            },
        ) {
            self.segment_count = self.segment_count.saturating_sub(replaced.segment_count);
        }
        self.segment_count = self.segment_count.saturating_add(segment_count);
        while self.entries.len() > MSDF_JOB_CACHE_MAX_ENTRIES
            || self.segment_count > MSDF_JOB_CACHE_MAX_SEGMENTS
        {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| *key)
            else {
                break;
            };
            if let Some(removed) = self.entries.remove(&oldest) {
                self.segment_count = self.segment_count.saturating_sub(removed.segment_count);
            }
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.segment_count = 0;
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

#[derive(Debug, Clone, PartialEq)]
pub struct TextLayout {
    pub measurement: TextMeasurement,
    pub glyphs: Vec<TextGlyph>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextGlyph {
    pub key: GlyphKey,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AtlasConfig {
    pub width: u32,
    pub height: u32,
    pub px_range: u32,
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
pub struct GlyphAtlas {
    config: AtlasConfig,
    entries: HashMap<GlyphKey, AtlasEntry>,
    cursor_x: u32,
    cursor_y: u32,
    row_height: u32,
}

impl GlyphAtlas {
    pub fn new(config: AtlasConfig) -> Self {
        Self {
            config,
            entries: HashMap::new(),
            cursor_x: 0,
            cursor_y: 0,
            row_height: 0,
        }
    }

    pub fn config(&self) -> AtlasConfig {
        self.config
    }

    pub fn used_size(&self) -> [u32; 2] {
        [
            self.config.width,
            self.cursor_y
                .saturating_add(self.row_height)
                .min(self.config.height),
        ]
    }

    pub fn entry(&self, key: &GlyphKey) -> Option<AtlasEntry> {
        self.entries.get(key).copied()
    }

    pub fn ensure_glyph(&mut self, key: GlyphKey, size: [u32; 2]) -> Option<AtlasEntry> {
        self.ensure_glyph_with_padding(key, size, self.config.px_range.max(1))
    }

    fn ensure_glyph_with_padding(
        &mut self,
        key: GlyphKey,
        size: [u32; 2],
        padding: u32,
    ) -> Option<AtlasEntry> {
        if let Some(entry) = self.entry(&key) {
            return Some(entry);
        }
        let width = size[0].saturating_add(padding * 2).max(1);
        let height = size[1].saturating_add(padding * 2).max(1);
        if width > self.config.width || height > self.config.height {
            return None;
        }
        if self.cursor_x + width > self.config.width {
            self.cursor_x = 0;
            self.cursor_y = self.cursor_y.saturating_add(self.row_height);
            self.row_height = 0;
        }
        if self.cursor_y + height > self.config.height {
            return None;
        }
        let entry = AtlasEntry {
            origin: [self.cursor_x + padding, self.cursor_y + padding],
            size,
            uv_min: [
                (self.cursor_x + padding) as f32 / self.config.width as f32,
                (self.cursor_y + padding) as f32 / self.config.height as f32,
            ],
            uv_max: [
                (self.cursor_x + padding + size[0]) as f32 / self.config.width as f32,
                (self.cursor_y + padding + size[1]) as f32 / self.config.height as f32,
            ],
        };
        self.cursor_x += width;
        self.row_height = self.row_height.max(height);
        self.entries.insert(key, entry);
        Some(entry)
    }
}

#[derive(Debug, Clone)]
pub struct AlphaAtlasRender {
    pub atlas: GlyphAtlas,
    pub pixels: Vec<u8>,
    pub instances: Vec<GpuGlyphInstance>,
    pub glyph_count: usize,
}

#[cfg(feature = "experimental-msdf")]
#[derive(Debug, Clone)]
pub struct GpuHybridAtlasRender {
    pub atlas: GlyphAtlas,
    pub pixels: Vec<u8>,
    pub instances: Vec<GpuGlyphInstance>,
    pub jobs: Vec<GpuMsdfJob>,
    pub segments: Vec<GpuMsdfSegment>,
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

fn write_glyph_to_atlas(
    pixels: &mut [u8],
    config: AtlasConfig,
    entry: AtlasEntry,
    image: &SwashImage,
) {
    match image.content {
        SwashContent::Mask => {
            for y in 0..image.placement.height as usize {
                for x in 0..image.placement.width as usize {
                    let src = y * image.placement.width as usize + x;
                    let dst_x = entry.origin[0] as usize + x;
                    let dst_y = entry.origin[1] as usize + y;
                    let dst = (dst_y * config.width as usize + dst_x) * 4;
                    let alpha = image.data[src];
                    pixels[dst] = 255;
                    pixels[dst + 1] = 255;
                    pixels[dst + 2] = 255;
                    pixels[dst + 3] = alpha;
                }
            }
        }
        SwashContent::Color => {
            for y in 0..image.placement.height as usize {
                for x in 0..image.placement.width as usize {
                    let src = (y * image.placement.width as usize + x) * 4;
                    let dst_x = entry.origin[0] as usize + x;
                    let dst_y = entry.origin[1] as usize + y;
                    let dst = (dst_y * config.width as usize + dst_x) * 4;
                    pixels[dst..dst + 4].copy_from_slice(&image.data[src..src + 4]);
                }
            }
        }
        SwashContent::SubpixelMask => {
            for y in 0..image.placement.height as usize {
                for x in 0..image.placement.width as usize {
                    let src = (y * image.placement.width as usize + x) * 3;
                    let dst_x = entry.origin[0] as usize + x;
                    let dst_y = entry.origin[1] as usize + y;
                    let dst = (dst_y * config.width as usize + dst_x) * 4;
                    let alpha = image.data[src]
                        .max(image.data[src + 1])
                        .max(image.data[src + 2]);
                    pixels[dst] = 255;
                    pixels[dst + 1] = 255;
                    pixels[dst + 2] = 255;
                    pixels[dst + 3] = alpha;
                }
            }
        }
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
    fn alpha_atlas_preserves_color_glyph_pixels() {
        let mut image = SwashImage::new();
        image.content = SwashContent::Color;
        image.placement.width = 2;
        image.placement.height = 1;
        image.data = vec![255, 0, 0, 200, 0, 128, 255, 180];
        let entry = AtlasEntry {
            origin: [1, 1],
            size: [2, 1],
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
        };
        let config = AtlasConfig {
            width: 4,
            height: 4,
            px_range: 1,
        };
        let mut pixels = vec![0_u8; config.width as usize * config.height as usize * 4];

        write_glyph_to_atlas(&mut pixels, config, entry, &image);

        let first = ((config.width + 1) * 4) as usize;
        let second = first + 4;
        assert_eq!(&pixels[first..first + 4], &[255, 0, 0, 200]);
        assert_eq!(&pixels[second..second + 4], &[0, 128, 255, 180]);
    }

    #[test]
    fn raster_keys_retain_subpixel_position() {
        let mut system = TextSystem::new();
        let mut first = TextLayoutRequest::new("A");
        first.origin = [0.1, 0.0];
        let mut second = first.clone();
        second.origin = [0.6, 0.0];

        let first_glyph = system.layout(&first).glyphs[0].clone();
        let second_glyph = system.layout(&second).glyphs[0].clone();

        assert_ne!(first_glyph.key, second_glyph.key);
        assert_eq!(first_glyph.x.fract(), 0.0);
        assert_eq!(second_glyph.x.fract(), 0.0);
    }

    #[test]
    fn alpha_instances_use_integer_raster_positions() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("AV\nSharp text");
        request.font_size = 31.0;
        request.origin = [0.6, 0.3];
        let layout = system.layout(&request);
        let render = system.render_alpha_atlas(&layout, AtlasConfig::default(), 128);

        assert!(!render.instances.is_empty());
        assert!(render.instances.iter().all(|instance| {
            instance.rect[0].fract() == 0.0 && instance.rect[1].fract() == 0.0
        }));
    }

    #[test]
    fn subpixel_rasters_change_without_changing_layout_measurement() {
        let mut system = TextSystem::new();
        let mut first = TextLayoutRequest::new("A");
        first.font_size = 31.0;
        first.origin = [0.0, 0.0];
        let mut second = first.clone();
        second.origin = [0.5, 0.0];

        let first_layout = system.layout(&first);
        let second_layout = system.layout(&second);
        assert_eq!(first_layout.measurement, second_layout.measurement);
        assert_ne!(first_layout.glyphs[0].key, second_layout.glyphs[0].key);

        let first_image = system
            .glyph_image(first_layout.glyphs[0].key)
            .expect("first glyph raster");
        let second_image = system
            .glyph_image(second_layout.glyphs[0].key)
            .expect("second glyph raster");
        assert_ne!(first_image.data, second_image.data);
    }

    #[cfg(feature = "experimental-msdf")]
    #[test]
    fn hybrid_atlas_uses_msdf_for_outline_glyphs() {
        let mut system = TextSystem::new();
        let layout = system.layout(&TextLayoutRequest::new("A"));
        let render =
            system.render_gpu_hybrid_atlas(&layout, AtlasConfig::default(), 16, 4096, 4096);

        assert!(render.instances.iter().any(is_msdf_instance));
        assert!(
            render.segments.iter().any(|segment| segment.channels != 7),
            "corner glyphs must use edge-colored channel masks"
        );
    }

    #[cfg(feature = "experimental-msdf")]
    #[test]
    fn repeated_large_glyphs_share_one_msdf_generation_job() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("W".repeat(128));
        request.font_size = 180.0;
        let layout = system.layout(&request);
        let render = system.render_gpu_hybrid_atlas(
            &layout,
            AtlasConfig {
                width: 512,
                height: 512,
                px_range: 8,
            },
            128,
            4096,
            512 * 512,
        );

        assert_eq!(render.instances.len(), 128);
        assert_eq!(render.glyph_count, 128);
        assert_eq!(render.jobs.len(), 1);
        assert_eq!(render.jobs[0].pixel_range[1], render.msdf_pixel_count);
        assert!(render.instances.iter().all(is_msdf_instance));
    }

    #[cfg(feature = "experimental-msdf")]
    #[test]
    fn many_large_glyphs_have_disjoint_generation_regions() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789");
        request.font_size = 144.0;
        let layout = system.layout(&request);
        let render = system.render_gpu_hybrid_atlas(
            &layout,
            AtlasConfig {
                width: 2048,
                height: 2048,
                px_range: 12,
            },
            64,
            65_536,
            2048 * 2048,
        );

        assert!(render.jobs.len() >= 30);
        for (index, job) in render.jobs.iter().enumerate() {
            assert_eq!(
                job.pixel_range[0],
                render.jobs[..index]
                    .iter()
                    .map(|job| job.pixel_range[1])
                    .sum()
            );
            for other in render.jobs.iter().skip(index + 1) {
                let [x, y, width, height] = job.atlas_rect;
                let [other_x, other_y, other_width, other_height] = other.atlas_rect;
                assert!(
                    x + width <= other_x
                        || other_x + other_width <= x
                        || y + height <= other_y
                        || other_y + other_height <= y,
                    "MSDF generation jobs must never write overlapping atlas regions"
                );
            }
        }
    }

    #[cfg(feature = "experimental-msdf")]
    #[test]
    fn msdf_job_cache_is_bounded_during_font_size_animation() {
        let mut system = TextSystem::new();
        let config = AtlasConfig::default();
        for frame in 0..(MSDF_JOB_CACHE_MAX_ENTRIES * 2) {
            let mut request = TextLayoutRequest::new("Animated");
            request.font_size = 20.0 + frame as f32 * 0.25;
            let layout = system.layout(&request);
            let key = layout.glyphs[0].key.msdf_key();
            let _ = system.glyph_msdf_job(key, config);
        }

        assert!(system.msdf_job_cache.entries.len() <= MSDF_JOB_CACHE_MAX_ENTRIES);
        assert!(system.msdf_job_cache.segment_count <= MSDF_JOB_CACHE_MAX_SEGMENTS);
    }

    #[cfg(feature = "experimental-msdf")]
    #[test]
    fn long_animated_large_text_sequence_stays_within_frame_budgets() {
        let mut system = TextSystem::new();
        let config = AtlasConfig {
            width: 2048,
            height: 2048,
            px_range: 12,
        };
        let max_segments = 32_768;
        let max_pixels = 1_500_000;
        let content = "LUMENLARGETYPE0123456789".repeat(4);

        for frame in 0..240 {
            let phase = frame as f32 * 0.071;
            let mut request = TextLayoutRequest::new(content.clone());
            request.font_size = 96.0 + phase.sin().abs() * 112.0;
            request.origin = [phase.cos() * 4.0 + 4.0, 240.0 + phase.sin() * 20.0];
            request.max_width = Some(1800.0);
            let layout = system.layout(&request);
            let expected_glyphs = layout.glyphs.len().min(128);
            let render =
                system.render_gpu_hybrid_atlas(&layout, config, 128, max_segments, max_pixels);

            assert_eq!(render.glyph_count, expected_glyphs, "frame {frame}");
            assert_eq!(render.instances.len(), expected_glyphs, "frame {frame}");
            assert!(render.segments.len() <= max_segments, "frame {frame}");
            assert!(render.msdf_pixel_count <= max_pixels, "frame {frame}");
            assert!(render.jobs.len() <= 22, "frame {frame}");
            assert_eq!(
                render
                    .jobs
                    .iter()
                    .map(|job| job.pixel_range[1])
                    .sum::<u32>(),
                render.msdf_pixel_count,
                "frame {frame}"
            );
        }

        assert!(system.msdf_job_cache.entries.len() <= MSDF_JOB_CACHE_MAX_ENTRIES);
        assert!(system.msdf_job_cache.segment_count <= MSDF_JOB_CACHE_MAX_SEGMENTS);
    }

    #[cfg(feature = "experimental-msdf")]
    #[test]
    fn msdf_job_cache_accounts_for_generation_config() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("B");
        request.font_size = 96.0;
        let key = system.layout(&request).glyphs[0].key.msdf_key();
        let narrow = system
            .glyph_msdf_job(
                key,
                AtlasConfig {
                    width: 512,
                    height: 512,
                    px_range: 2,
                },
            )
            .unwrap();
        let wide = system
            .glyph_msdf_job(
                key,
                AtlasConfig {
                    width: 512,
                    height: 512,
                    px_range: 16,
                },
            )
            .unwrap();

        assert!(wide.placement.width > narrow.placement.width);
        assert!(wide.placement.height > narrow.placement.height);
        assert_eq!(system.msdf_job_cache.entries.len(), 2);
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
    fn msdf_instances_restore_normalized_subpixel_position() {
        let mut system = TextSystem::new();
        let mut first = TextLayoutRequest::new("A");
        first.origin = [0.1, 0.0];
        let mut second = first.clone();
        second.origin = [0.6, 0.0];

        let first_layout = system.layout(&first);
        let second_layout = system.layout(&second);
        let first_render =
            system.render_gpu_hybrid_atlas(&first_layout, AtlasConfig::default(), 1, 4096, 4096);
        let second_render =
            system.render_gpu_hybrid_atlas(&second_layout, AtlasConfig::default(), 1, 4096, 4096);

        assert!(is_msdf_instance(&first_render.instances[0]));
        assert!(is_msdf_instance(&second_render.instances[0]));
        assert_eq!(
            second_render.instances[0].rect[0] - first_render.instances[0].rect[0],
            0.5
        );
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
        let msdf_key = glyph_key.msdf_key();
        assert_ne!(glyph_key, msdf_key);

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
        assert!(render.atlas.entry(&msdf_key).is_none());
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
        assert_eq!(std::mem::size_of::<GpuTextGlobals>(), 16);
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
