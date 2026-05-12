//! Text layout and GPU-facing text data for Lumen.
//!
//! This crate deliberately owns text shaping, measurement, atlas bookkeeping,
//! and WGSL-facing buffer formats. Lumen nodes should depend on this layer
//! rather than coupling directly to a specific renderer such as glyphon.

use std::collections::HashMap;

use cosmic_text::{
    Align, Attrs, Buffer, CacheKey, CacheKeyFlags, Family, FontSystem, Metrics, Shaping, Style,
    SubpixelBin, SwashCache, SwashContent, SwashImage, Weight, Wrap,
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

const DEFAULT_ROBOTO_BYTES: &[u8] = include_bytes!("../../lumen/assets/roboto/Roboto-Regular.ttf");

#[derive(Debug)]
pub struct TextSystem {
    font_system: FontSystem,
    swash_cache: SwashCache,
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
            #[cfg(feature = "experimental-msdf")]
            msdf_job_cache: HashMap::new(),
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
                let mut cache_key = physical.cache_key;
                cache_key.x_bin = SubpixelBin::Zero;
                cache_key.y_bin = SubpixelBin::Zero;
                let x = request.origin[0] + glyph.x + (glyph.font_size * glyph.x_offset);
                let y =
                    request.origin[1] + run.line_y + glyph.y - (glyph.font_size * glyph.y_offset);
                glyphs.push(TextGlyph {
                    key: GlyphKey(cache_key),
                    x,
                    y,
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
        let image = self.glyph_image(glyph.key)?;
        let glyph_size = [image.placement.width, image.placement.height];
        let entry = atlas.ensure_glyph(glyph.key, glyph_size)?;
        write_glyph_to_atlas(pixels, config, entry, &image);
        Some(glyph_instance_for(glyph, entry, &image))
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
    db.load_font_data(DEFAULT_ROBOTO_BYTES.to_vec());
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    pub fn entry(&self, key: &GlyphKey) -> Option<AtlasEntry> {
        self.entries.get(key).copied()
    }

    pub fn ensure_glyph(&mut self, key: GlyphKey, size: [u32; 2]) -> Option<AtlasEntry> {
        if let Some(entry) = self.entry(&key) {
            return Some(entry);
        }
        let padding = self.config.px_range.max(1);
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
    fn raster_keys_ignore_subpixel_position() {
        let mut system = TextSystem::new();
        let mut first = TextLayoutRequest::new("A");
        first.origin = [0.1, 0.0];
        let mut second = first.clone();
        second.origin = [0.6, 0.0];

        let first_glyph = system.layout(&first).glyphs[0].clone();
        let second_glyph = system.layout(&second).glyphs[0].clone();

        assert_eq!(first_glyph.key, second_glyph.key);
        assert_ne!(first_glyph.x, second_glyph.x);
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

        assert_eq!(first_key, second_key);
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
        let msdf_key = glyph_key.msdf_key();
        assert_eq!(glyph_key, msdf_key);

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
    fn exposes_wgsl_shader_sources() {
        assert!(ALPHA_TEXT_SHADER.contains("textureSample"));
        #[cfg(feature = "experimental-msdf")]
        {
            assert!(MSDF_TEXT_SHADER.contains("median3"));
            assert!(MSDF_TEXT_SHADER.contains("textureSample"));
            assert!(MSDF_GENERATOR_SHADER.contains("pixel_jobs"));
        }
    }
}
