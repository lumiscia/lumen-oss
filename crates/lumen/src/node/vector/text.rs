use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

use crate::error::{LumenError, RenderError};
use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, FrameBinding, GpuCompileNode, GpuFrameBindNode,
};
use crate::node::{NodeId, NodeProperty, PortRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, lumen_macros::Node)]
#[node(
    kind = "text",
    label = "Text",
    description = "Produces a vector text layer for GPU rasterization.",
    category = "vector"
)]
pub struct Text {
    pub id: NodeId,
    #[property(kind = "string")]
    pub content: NodeProperty,
    #[property(kind = "string")]
    pub font_family: NodeProperty,
    #[property(kind = "float")]
    pub font_size: NodeProperty,
    #[property(kind = "int")]
    pub font_weight: NodeProperty,
    #[property(kind = "int")]
    pub font_style: NodeProperty,
    #[property(kind = "float")]
    pub max_width: NodeProperty,
    #[property(kind = "vec2")]
    pub position: NodeProperty,
    #[property(kind = "color")]
    pub color: NodeProperty,
    #[property(kind = "int")]
    pub alignment_horizontal: NodeProperty,
    #[property(kind = "int")]
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
            output_texture: _output_texture,
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
        request.origin = [position_x as f32, position_y as f32];
        let color_f32 = rgba8_to_f32(color);
        request.color = color_f32;
        request.align = match alignment_horizontal {
            TextAlignmentHorizontal::Center => lumen_text::TextAlign::Center,
            TextAlignmentHorizontal::Right => lumen_text::TextAlign::Right,
            TextAlignmentHorizontal::Justify => lumen_text::TextAlign::Justified,
            TextAlignmentHorizontal::Left => lumen_text::TextAlign::Left,
        };

        let cache_key = TextCacheKey {
            content,
            font_family,
            font_size_bits: font_size.to_bits(),
            font_weight: request.font_weight,
            font_style,
            max_width_bits: max_width.to_bits(),
            position_x_bits: request.origin[0].to_bits(),
            position_y_bits: request.origin[1].to_bits(),
            color,
            alignment_horizontal,
            alignment_vertical,
            atlas_width: atlas_size.width,
            atlas_height: atlas_size.height,
            output_width: size.width,
            output_height: size.height,
            max_glyphs: *max_glyphs,
        };
        if text_cache()?
            .get(&node_id.0)
            .is_some_and(|cached| cached == &cache_key)
        {
            return Ok(());
        }

        let mut text_system = text_system()?;
        let measurement = text_system.measure(&request);
        request.origin[1] = match alignment_vertical {
            TextAlignmentVertical::Top => request.origin[1],
            TextAlignmentVertical::Middle => {
                request.origin[1] + ((size.height as f32 - measurement.height) * 0.5).max(0.0)
            }
            TextAlignmentVertical::Bottom => {
                request.origin[1] + (size.height as f32 - measurement.height).max(0.0)
            }
        };
        let layout = text_system.layout(&request);
        let atlas = text_system.render_alpha_atlas(
            &layout,
            lumen_text::AtlasConfig {
                width: atlas_size.width,
                height: atlas_size.height,
                px_range: 1,
            },
            *max_glyphs,
        );
        let globals = lumen_text::GpuTextGlobals {
            target_size: [size.width as f32, size.height as f32],
            px_range: 1.0,
            glyph_count: atlas.glyph_count as u32,
        };

        bound.write_buffer(*globals_buffer, 0, bytemuck::bytes_of(&globals));
        if !atlas.instances.is_empty() {
            bound.write_buffer(*instances_buffer, 0, bytemuck::cast_slice(&atlas.instances));
        }
        bound.write_texture_rgba8(
            *atlas_texture,
            atlas.pixels,
            atlas_size.width * 4,
            atlas_size.height,
        );
        text_cache()?.insert(node_id.0, cache_key);
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TextCacheKey {
    content: String,
    font_family: String,
    font_size_bits: u32,
    font_weight: u16,
    font_style: TextFontStyle,
    max_width_bits: u32,
    position_x_bits: u32,
    position_y_bits: u32,
    color: [u8; 4],
    alignment_horizontal: TextAlignmentHorizontal,
    alignment_vertical: TextAlignmentVertical,
    atlas_width: u32,
    atlas_height: u32,
    output_width: u32,
    output_height: u32,
    max_glyphs: usize,
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

fn text_cache() -> crate::Result<std::sync::MutexGuard<'static, HashMap<u64, TextCacheKey>>> {
    static TEXT_CACHE: OnceLock<Mutex<HashMap<u64, TextCacheKey>>> = OnceLock::new();
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
