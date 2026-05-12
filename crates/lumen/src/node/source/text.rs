use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

use crate::error::{LumenError, RenderError};
use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, FrameBinding, GpuCompileNode, GpuFrameBindNode,
};
use crate::node::{NodeId, NodeProperty, PortRef};

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
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "text", name = "Text", category = "source")]
pub struct Text {
    pub id: NodeId,
    /// Text content to render.
    #[property(kind = "string", multiline, recommended_rows = 4)]
    pub content: NodeProperty,
    /// Font family name.
    #[property(kind = "string", format = "font_family")]
    pub font_family: NodeProperty,
    /// Font size in pixels.
    #[property(kind = "float", min = 1, step = 1)]
    pub font_size: NodeProperty,
    /// Font weight.
    #[property(kind = "int", min = 100, max = 900, step = 100)]
    pub font_weight: NodeProperty,
    /// Font style.
    #[property(kind = "enum", enum_type = TextFontStyle)]
    pub font_style: NodeProperty,
    /// Maximum line width in pixels. Use 0 for automatic width.
    #[property(kind = "float", min = 0, step = 1)]
    pub max_width: NodeProperty,
    /// Text origin in pixels.
    #[property(kind = "vec2")]
    pub position: NodeProperty,
    /// Text color.
    #[property(kind = "color")]
    pub color: NodeProperty,
    /// Horizontal text alignment.
    #[property(kind = "enum", enum_type = TextAlignmentHorizontal)]
    pub alignment_horizontal: NodeProperty,
    /// Vertical text alignment.
    #[property(kind = "enum", enum_type = TextAlignmentVertical)]
    pub alignment_vertical: NodeProperty,
}

impl Default for Text {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            content: NodeProperty::String(String::new()),
            font_family: NodeProperty::String(lumen_text::DEFAULT_FONT_FAMILY.to_string()),
            font_size: NodeProperty::Float(16.0),
            font_weight: NodeProperty::Int(400),
            font_style: NodeProperty::Int(TextFontStyle::Normal as i64),
            max_width: NodeProperty::Float(0.0),
            position: NodeProperty::Vec2((0.0, 0.0)),
            color: NodeProperty::Color([255, 255, 255, 255]),
            alignment_horizontal: NodeProperty::Int(TextAlignmentHorizontal::Left as i64),
            alignment_vertical: NodeProperty::Int(TextAlignmentVertical::Top as i64),
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

impl GpuFrameBindNode for Text {
    fn bind_gpu_frame(
        &self,
        ctx: &FrameBindContext<'_>,
        binding: &FrameBinding,
        bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        let FrameBinding::Text {
            node_id,
            content,
            font_family,
            font_size,
            font_weight,
            font_style,
            max_width,
            position,
            color,
            alignment_horizontal,
            alignment_vertical,
            atlas_texture,
            globals_buffer,
            instances_buffer,
            atlas_size,
            max_glyphs,
            size,
        } = binding
        else {
            return Ok(());
        };

        let content =
            content.resolve_string(*node_id, "content", &ctx.expr_context(*node_id, "content"))?;
        let font_family = font_family.resolve_string(
            *node_id,
            "font_family",
            &ctx.expr_context(*node_id, "font_family"),
        )?;
        let color = color.resolve_color(*node_id, "color", &ctx.expr_context(*node_id, "color"))?;
        let (position_x, position_y) = position.resolve_vec2(
            *node_id,
            "position",
            &ctx.expr_context(*node_id, "position"),
        )?;
        let font_size = font_size.resolve_float(
            *node_id,
            "font_size",
            &ctx.expr_context(*node_id, "font_size"),
        )? as f32;
        let max_width = max_width.resolve_float(
            *node_id,
            "max_width",
            &ctx.expr_context(*node_id, "max_width"),
        )? as f32;
        let alignment_horizontal =
            TextAlignmentHorizontal::from_int(alignment_horizontal.resolve_int(
                *node_id,
                "alignment_horizontal",
                &ctx.expr_context(*node_id, "alignment_horizontal"),
            )?);
        let alignment_vertical = TextAlignmentVertical::from_int(alignment_vertical.resolve_int(
            *node_id,
            "alignment_vertical",
            &ctx.expr_context(*node_id, "alignment_vertical"),
        )?);
        let font_weight = font_weight.resolve_int(
            *node_id,
            "font_weight",
            &ctx.expr_context(*node_id, "font_weight"),
        )?;
        let font_style = TextFontStyle::from_int(font_style.resolve_int(
            *node_id,
            "font_style",
            &ctx.expr_context(*node_id, "font_style"),
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
            atlas_width: atlas_size.width,
            atlas_height: atlas_size.height,
            max_glyphs: *max_glyphs,
        };
        let frame_key = TextFrameCacheKey {
            atlas_key: atlas_key.clone(),
            position_x_bits: (position_x as f32).to_bits(),
            position_y_bits: (position_y as f32).to_bits(),
            color,
            alignment_vertical,
            output_width: size.width,
            output_height: size.height,
        };
        if text_cache()?
            .get(&node_id.0)
            .is_some_and(|cached| cached.frame_key.as_ref() == Some(&frame_key))
        {
            return Ok(());
        }

        let atlas_config = lumen_text::AtlasConfig {
            width: atlas_size.width,
            height: atlas_size.height,
            ..lumen_text::AtlasConfig::default()
        };
        let mut cache = text_cache()?;
        let cached = cache.entry(node_id.0).or_default();
        let atlas_changed = cached.atlas_key.as_ref() != Some(&atlas_key);
        if atlas_changed {
            let layout = text_system.layout(&request);
            let atlas = text_system.render_alpha_atlas(&layout, atlas_config, *max_glyphs);
            cached.atlas_key = Some(atlas_key.clone());
            cached.base_instances = atlas.instances;
            cached.glyph_count = atlas.glyph_count;
            cached.measurement_height = layout.measurement.height;
            bound.write_texture_rgba8(
                *atlas_texture,
                atlas.pixels,
                atlas_size.width * 4,
                atlas_size.height,
            );
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
            target_size: [size.width as f32, size.height as f32],
            px_range: atlas_config.px_range as f32,
            glyph_count: cached.glyph_count as u32,
        };

        bound.write_buffer(*globals_buffer, 0, bytemuck::bytes_of(&globals));
        if !instances.is_empty() {
            bound.write_buffer(*instances_buffer, 0, bytemuck::cast_slice(&instances));
        }
        cached.frame_key = Some(frame_key);
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
}

fn rgba8_to_f32(color: [u8; 4]) -> [f32; 4] {
    [
        f32::from(color[0]) / 255.0,
        f32::from(color[1]) / 255.0,
        f32::from(color[2]) / 255.0,
        f32::from(color[3]) / 255.0,
    ]
}
