use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
    sync::{Mutex, OnceLock},
};

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
use std::time::Instant;

use crate::error::{LumenError, RenderError};
use crate::gpu::{BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuFrameBinding};
use crate::node::{Deferred, NodeId, NodeParams, PortRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, lumen_macros::NodeEnum)]
#[repr(i64)]
pub enum TextFontStyle {
    Normal = 0,
    Italic = 1,
    Oblique = 2,
}

impl TextFontStyle {
    pub fn from_int(value: i64) -> Self {
        match value {
            1 => Self::Italic,
            2 => Self::Oblique,
            _ => Self::Normal,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, lumen_macros::NodeEnum)]
#[repr(i64)]
pub enum TextAlignmentHorizontal {
    Left = 0,
    Center = 1,
    Right = 2,
    Justify = 3,
}

impl TextAlignmentHorizontal {
    pub fn from_int(value: i64) -> Self {
        match value {
            1 => Self::Center,
            2 => Self::Right,
            3 => Self::Justify,
            _ => Self::Left,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, lumen_macros::NodeEnum)]
#[repr(i64)]
pub enum TextAlignmentVertical {
    Top = 0,
    Middle = 1,
    Bottom = 2,
}

impl TextAlignmentVertical {
    pub fn from_int(value: i64) -> Self {
        match value {
            1 => Self::Middle,
            2 => Self::Bottom,
            _ => Self::Top,
        }
    }
}

/// Produces a text raster source.
#[derive(Debug, Clone, lumen_macros::NodeParams)]
#[params(evaluated = EvaluatedTextParams)]
#[cfg_attr(feature = "json", derive(serde::Deserialize), serde(default))]
pub struct TextParams {
    /// Text content to render.
    #[param(kind = "string", multiline, recommended_rows = 4)]
    pub content: Deferred<String>,
    /// Font family name.
    #[param(kind = "string", format = "font_family")]
    pub font_family: Deferred<String>,
    /// Font size in pixels.
    #[param(kind = "float", min = 1, step = 1)]
    pub font_size: Deferred<f64>,
    /// Font weight.
    #[param(kind = "int", min = 100, max = 900, step = 100)]
    pub font_weight: Deferred<i64>,
    /// Font style.
    #[param(kind = "enum", enum_type = TextFontStyle)]
    pub font_style: Deferred<i64>,
    /// Maximum line width in pixels. Use 0 for automatic width.
    #[param(kind = "float", min = 0, step = 1)]
    pub max_width: Deferred<f64>,
    /// Text origin in pixels.
    #[param(kind = "vec2")]
    pub position: Deferred<(f64, f64)>,
    /// Text color.
    #[param(kind = "color")]
    pub color: Deferred<[u8; 4]>,
    /// Horizontal text alignment.
    #[param(kind = "enum", enum_type = TextAlignmentHorizontal)]
    pub alignment_horizontal: Deferred<i64>,
    /// Vertical text alignment.
    #[param(kind = "enum", enum_type = TextAlignmentVertical)]
    pub alignment_vertical: Deferred<i64>,
}

impl Default for TextParams {
    fn default() -> Self {
        Self {
            content: Deferred::value(String::new()),
            font_family: Deferred::value(lumen_text::DEFAULT_FONT_FAMILY.to_string()),
            font_size: Deferred::value(16.0),
            font_weight: Deferred::value(400),
            font_style: Deferred::value(TextFontStyle::Normal as i64),
            max_width: Deferred::value(0.0),
            position: Deferred::value((0.0, 0.0)),
            color: Deferred::value([255, 255, 255, 255]),
            alignment_horizontal: Deferred::value(TextAlignmentHorizontal::Left as i64),
            alignment_vertical: Deferred::value(TextAlignmentVertical::Top as i64),
        }
    }
}

/// Produces a text raster source.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "text", name = "Text", category = "source")]
pub struct Text {
    pub id: NodeId,
    #[params]
    pub params: TextParams,
}

impl Default for Text {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: TextParams::default(),
        }
    }
}

impl GpuCompileNode for Text {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        crate::node::vector::renderer::VectorRenderer::new(ctx).compile_text(self, port)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TextFrameBinding {
    pub(crate) node_id: NodeId,
    pub(crate) content: Deferred<String>,
    pub(crate) font_family: Deferred<String>,
    pub(crate) font_size: Deferred<f64>,
    pub(crate) font_weight: Deferred<i64>,
    pub(crate) font_style: Deferred<i64>,
    pub(crate) max_width: Deferred<f64>,
    pub(crate) position: Deferred<(f64, f64)>,
    pub(crate) color: Deferred<[u8; 4]>,
    pub(crate) alignment_horizontal: Deferred<i64>,
    pub(crate) alignment_vertical: Deferred<i64>,
    pub(crate) atlas_texture: lumen_gpu::TextureId,
    pub(crate) globals_buffer: lumen_gpu::BufferId,
    pub(crate) instances_buffer: lumen_gpu::BufferId,
    pub(crate) atlas_size: lumen_gpu::Size,
    pub(crate) max_glyphs: usize,
    pub(crate) size: lumen_gpu::Size,
}

impl GpuFrameBinding for TextFrameBinding {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let trace_started = crate::log_level_enabled(tracing::Level::TRACE).then(trace_now_ms);
        let content = self.content.resolve_string(
            self.node_id,
            "content",
            &ctx.expr_context(self.node_id, "content"),
        )?;
        let font_family = self.font_family.resolve_string(
            self.node_id,
            "font_family",
            &ctx.expr_context(self.node_id, "font_family"),
        )?;
        let color = self.color.resolve_color(
            self.node_id,
            "color",
            &ctx.expr_context(self.node_id, "color"),
        )?;
        let (position_x, position_y) = self.position.resolve_vec2(
            self.node_id,
            "position",
            &ctx.expr_context(self.node_id, "position"),
        )?;
        let font_size = self.font_size.resolve_float(
            self.node_id,
            "font_size",
            &ctx.expr_context(self.node_id, "font_size"),
        )? as f32;
        let max_width = self.max_width.resolve_float(
            self.node_id,
            "max_width",
            &ctx.expr_context(self.node_id, "max_width"),
        )? as f32;
        let alignment_horizontal =
            TextAlignmentHorizontal::from_int(self.alignment_horizontal.resolve_int(
                self.node_id,
                "alignment_horizontal",
                &ctx.expr_context(self.node_id, "alignment_horizontal"),
            )?);
        let alignment_vertical =
            TextAlignmentVertical::from_int(self.alignment_vertical.resolve_int(
                self.node_id,
                "alignment_vertical",
                &ctx.expr_context(self.node_id, "alignment_vertical"),
            )?);
        let font_weight = self.font_weight.resolve_int(
            self.node_id,
            "font_weight",
            &ctx.expr_context(self.node_id, "font_weight"),
        )?;
        let font_style = TextFontStyle::from_int(self.font_style.resolve_int(
            self.node_id,
            "font_style",
            &ctx.expr_context(self.node_id, "font_style"),
        )?);

        let mut request = lumen_text::TextLayoutRequest::new(content.clone());
        request.font_family = font_family.clone();
        request.font_size = font_size;
        request.font_weight = font_weight.clamp(1, 1000) as u16;
        request.font_style = match font_style {
            TextFontStyle::Italic => lumen_text::TextFontStyle::Italic,
            TextFontStyle::Oblique => lumen_text::TextFontStyle::Oblique,
            TextFontStyle::Normal => lumen_text::TextFontStyle::Normal,
        };
        request.max_width = (max_width > 0.0).then_some(max_width);
        request.origin = [0.0, 0.0];
        let color_f32 = rgba8_to_f32(color);
        request.color = [1.0; 4];
        request.align = match alignment_horizontal {
            TextAlignmentHorizontal::Center => lumen_text::TextAlign::Center,
            TextAlignmentHorizontal::Right => lumen_text::TextAlign::Right,
            TextAlignmentHorizontal::Justify => lumen_text::TextAlign::Justified,
            TextAlignmentHorizontal::Left => lumen_text::TextAlign::Left,
        };

        let mut text_system = text_system()?;
        load_font_family(&mut text_system, ctx, &font_family)?;

        let atlas_key = TextAtlasCacheKey {
            content: content.clone(),
            font_family: font_family.clone(),
            font_size_bits: font_size.to_bits(),
            font_weight: request.font_weight,
            font_style,
            max_width_bits: max_width.to_bits(),
            alignment_horizontal,
            atlas_width: self.atlas_size.width,
            atlas_height: self.atlas_size.height,
            max_glyphs: self.max_glyphs,
        };
        let frame_key = TextFrameCacheKey {
            atlas_key: atlas_key.clone(),
            position_x_bits: (position_x as f32).to_bits(),
            position_y_bits: (position_y as f32).to_bits(),
            color,
            alignment_vertical,
            output_width: self.size.width,
            output_height: self.size.height,
        };
        if text_cache()?
            .get(&self.node_id.0)
            .is_some_and(|cached| cached.frame_key.as_ref() == Some(&frame_key))
        {
            return Ok(());
        }

        let atlas_config = lumen_text::AtlasConfig {
            width: self.atlas_size.width,
            height: self.atlas_size.height,
            px_range: 1,
        };
        let mut cache = text_cache()?;
        let cached = cache.entry(self.node_id.0).or_default();
        let atlas_changed = cached.atlas_key.as_ref() != Some(&atlas_key);
        let mut layout_ms = 0.0;
        let mut atlas_ms = 0.0;
        let mut upload_ms = 0.0;
        if atlas_changed {
            let layout_started = trace_started.map(|_| trace_now_ms());
            let layout = text_system.layout(&request);
            if let Some(started) = layout_started {
                layout_ms = trace_now_ms() - started;
            }
            let atlas_started = trace_started.map(|_| trace_now_ms());
            let atlas = text_system.render_alpha_atlas(&layout, atlas_config, self.max_glyphs);
            if let Some(started) = atlas_started {
                atlas_ms = trace_now_ms() - started;
            }
            cached.atlas_key = Some(atlas_key.clone());
            cached.base_instances = atlas.instances;
            cached.glyph_count = atlas.glyph_count;
            cached.measurement_height = layout.measurement.height;
            let used_size = atlas.atlas.used_size();
            let upload_height = used_size[1].max(1);
            let upload_len = self.atlas_size.width as usize * upload_height as usize * 4;
            let upload_pixels = atlas.pixels[..upload_len.min(atlas.pixels.len())].to_vec();
            let upload_started = trace_started.map(|_| trace_now_ms());
            bound.write_texture_rgba8_region(
                self.atlas_texture,
                upload_pixels,
                [0, 0, 0],
                lumen_gpu::Size::new(self.atlas_size.width, upload_height),
                self.atlas_size.width * 4,
                upload_height,
            );
            if let Some(started) = upload_started {
                upload_ms = trace_now_ms() - started;
            }
        }

        let y_offset = match alignment_vertical {
            TextAlignmentVertical::Top => position_y as f32,
            TextAlignmentVertical::Middle => position_y as f32 - cached.measurement_height * 0.5,
            TextAlignmentVertical::Bottom => position_y as f32 - cached.measurement_height,
        };
        let instances = positioned_text_instances(
            &cached.base_instances,
            position_x as f32,
            y_offset,
            color_f32,
        );
        let globals = lumen_text::GpuTextGlobals {
            target_size: [self.size.width as f32, self.size.height as f32],
            px_range: atlas_config.px_range as f32,
            glyph_count: cached.glyph_count as u32,
        };

        bound.write_buffer(self.globals_buffer, 0, bytemuck::bytes_of(&globals));
        if !instances.is_empty() {
            bound.write_buffer(self.instances_buffer, 0, bytemuck::cast_slice(&instances));
        }
        cached.frame_key = Some(frame_key);
        if let Some(started) = trace_started {
            trace_text_bind(
                self.node_id,
                atlas_changed,
                cached.glyph_count,
                layout_ms,
                atlas_ms,
                upload_ms,
                trace_now_ms() - started,
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
struct CachedText {
    atlas_key: Option<TextAtlasCacheKey>,
    frame_key: Option<TextFrameCacheKey>,
    base_instances: Vec<lumen_text::GpuGlyphInstance>,
    glyph_count: usize,
    measurement_height: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TextAtlasCacheKey {
    content: String,
    font_family: String,
    font_size_bits: u32,
    font_weight: u16,
    font_style: TextFontStyle,
    max_width_bits: u32,
    alignment_horizontal: TextAlignmentHorizontal,
    atlas_width: u32,
    atlas_height: u32,
    max_glyphs: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TextFrameCacheKey {
    atlas_key: TextAtlasCacheKey,
    position_x_bits: u32,
    position_y_bits: u32,
    color: [u8; 4],
    alignment_vertical: TextAlignmentVertical,
    output_width: u32,
    output_height: u32,
}

fn positioned_text_instances(
    base_instances: &[lumen_text::GpuGlyphInstance],
    x_offset: f32,
    y_offset: f32,
    color: [f32; 4],
) -> Vec<lumen_text::GpuGlyphInstance> {
    base_instances
        .iter()
        .map(|instance| {
            let mut instance = *instance;
            instance.rect[0] += x_offset;
            instance.rect[1] += y_offset;
            instance.color = color;
            instance
        })
        .collect()
}

fn text_system() -> crate::Result<std::sync::MutexGuard<'static, lumen_text::TextSystem>> {
    static TEXT_SYSTEM: OnceLock<Mutex<lumen_text::TextSystem>> = OnceLock::new();
    TEXT_SYSTEM
        .get_or_init(|| Mutex::new(lumen_text::TextSystem::new()))
        .lock()
        .map_err(|_| {
            LumenError::Render(RenderError::Gpu {
                details: "text system lock was poisoned".to_string(),
            })
        })
}

fn load_font_family(
    text_system: &mut lumen_text::TextSystem,
    ctx: &FrameBindContext<'_>,
    font_family: &str,
) -> crate::Result<()> {
    if font_family.is_empty() {
        return Ok(());
    }
    let Some(store) = ctx.media() else {
        return Ok(());
    };

    let Some(resolver) = store.get_font_resolver(font_family) else {
        return Ok(());
    };
    let resolver_id = resolver.id().to_string();
    if loaded_fonts()?
        .get(font_family)
        .is_some_and(|loaded_id| loaded_id == &resolver_id)
    {
        return Ok(());
    }
    for data in resolver.data().map_err(LumenError::Media)? {
        text_system.load_font_data(data);
    }
    loaded_fonts()?.insert(font_family.to_string(), resolver_id);
    text_cache()?.clear();
    Ok(())
}

fn loaded_fonts() -> crate::Result<std::sync::MutexGuard<'static, HashMap<String, String>>> {
    static LOADED_FONTS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    LOADED_FONTS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| {
            LumenError::Render(RenderError::Gpu {
                details: "loaded fonts lock was poisoned".to_string(),
            })
        })
}

fn text_cache() -> crate::Result<std::sync::MutexGuard<'static, HashMap<u64, CachedText>>> {
    static TEXT_CACHE: OnceLock<Mutex<HashMap<u64, CachedText>>> = OnceLock::new();
    TEXT_CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| {
            LumenError::Render(RenderError::Gpu {
                details: "text cache lock was poisoned".to_string(),
            })
        })
}

pub(crate) fn clear_text_cache_for(node_id: NodeId) {
    if let Ok(mut cache) = text_cache() {
        cache.remove(&node_id.0);
    }
    if crate::log_level_enabled(tracing::Level::TRACE) {
        tracing::trace!(
            target: "lumen_text",
            node_id = node_id.0,
            "text cache clear"
        );
    }
}

fn trace_text_bind(
    node_id: NodeId,
    atlas_changed: bool,
    glyph_count: usize,
    layout_ms: f64,
    atlas_ms: f64,
    upload_ms: f64,
    total_ms: f64,
) {
    let should_log = atlas_changed || total_ms >= 1.0;
    if !should_log {
        return;
    }
    static BIND_COUNT: AtomicU64 = AtomicU64::new(0);
    static ATLAS_MISS_COUNT: AtomicU64 = AtomicU64::new(0);
    let bind_count = BIND_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    let atlas_miss_count = if atlas_changed {
        ATLAS_MISS_COUNT.fetch_add(1, Ordering::Relaxed) + 1
    } else {
        ATLAS_MISS_COUNT.load(Ordering::Relaxed)
    };
    tracing::trace!(
        target: "lumen_text",
        bind = bind_count,
        atlas_misses = atlas_miss_count,
        node_id = node_id.0,
        atlas_miss = atlas_changed,
        glyphs = glyph_count,
        layout_ms,
        atlas_ms,
        atlas_upload_ms = upload_ms,
        bind_ms = total_ms,
        "text bind"
    );
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
fn trace_now_ms() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
fn trace_now_ms() -> f64 {
    js_sys::Date::now()
}

fn rgba8_to_f32(color: [u8; 4]) -> [f32; 4] {
    [
        f32::from(color[0]) / 255.0,
        f32::from(color[1]) / 255.0,
        f32::from(color[2]) / 255.0,
        f32::from(color[3]) / 255.0,
    ]
}
