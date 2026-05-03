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
            font_family: NodeProperty::String("sans-serif".to_string()),
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
            font_size,
            max_width,
            position,
            color,
            alignment_horizontal,
            alignment_vertical,
            buffer,
            text_buffer,
        } = binding
        else {
            return Ok(());
        };
        let content =
            content.resolve_string(*node_id, "content", &ctx.expr_context(*node_id, "content"))?;
        let color = color.resolve_color(*node_id, "color", &ctx.expr_context(*node_id, "color"))?;
        let (position_x, position_y) = position.resolve_vec2(
            *node_id,
            "position",
            &ctx.expr_context(*node_id, "position"),
        )?;
        let params = super::renderer::TextParams {
            color: rgba8_to_f32(color),
            position: [position_x as f32, position_y as f32],
            font_size: font_size.resolve_float(
                *node_id,
                "font_size",
                &ctx.expr_context(*node_id, "font_size"),
            )? as f32,
            max_width: max_width.resolve_float(
                *node_id,
                "max_width",
                &ctx.expr_context(*node_id, "max_width"),
            )? as f32,
            content_len: content.chars().count() as u32,
            line_count: content.lines().count().max(1) as u32,
            alignment_horizontal: TextAlignmentHorizontal::from_int(
                alignment_horizontal.resolve_int(
                    *node_id,
                    "alignment_horizontal",
                    &ctx.expr_context(*node_id, "alignment_horizontal"),
                )?,
            ) as u32,
            alignment_vertical: TextAlignmentVertical::from_int(alignment_vertical.resolve_int(
                *node_id,
                "alignment_vertical",
                &ctx.expr_context(*node_id, "alignment_vertical"),
            )?) as u32,
            _pad: [0; 4],
        };
        bound.write_buffer(*buffer, 0, bytemuck::bytes_of(&params));
        let mut chars = vec![0_u32; super::renderer::MAX_TEXT_CHARS];
        for (index, ch) in content.chars().take(chars.len()).enumerate() {
            chars[index] = ch as u32;
        }
        bound.write_buffer(*text_buffer, 0, bytemuck::cast_slice(&chars));
        Ok(())
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
