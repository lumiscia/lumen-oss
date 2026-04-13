#[cfg(feature = "embed-roboto")]
use skia_safe::textlayout::TypefaceFontProvider;
use skia_safe::{
    FontMgr, FontStyle,
    font_style::Weight,
    textlayout::{
        FontCollection, ParagraphBuilder, ParagraphStyle, TextAlign as ParagraphTextAlign,
        TextStyle as ParagraphTextStyle,
    },
};

use crate::{
    node::{
        NodeId, NodeProperty,
        pixel_utils::{ClearMode, render_to_surface_ephemeral, to_skia_color},
    },
    raster::{AlphaMode, RasterFrame, RectI},
    render::RenderContext,
};
use lumen_macros::{Node, node_impl};

#[cfg(feature = "embed-roboto")]
const EMBEDDED_ROBOTO_REGULAR: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/roboto/Roboto-Regular.ttf"
));

fn text_font_collection() -> FontCollection {
    let font_mgr = FontMgr::default();
    new_text_font_collection(&font_mgr)
}

fn new_text_font_collection(default_font_mgr: &FontMgr) -> FontCollection {
    let mut font_collection = FontCollection::new();
    font_collection.set_default_font_manager(default_font_mgr.clone(), None);
    #[cfg(feature = "embed-roboto")]
    attach_embedded_roboto(&mut font_collection, default_font_mgr);
    font_collection
}

#[cfg(feature = "embed-roboto")]
fn attach_embedded_roboto(font_collection: &mut FontCollection, default_font_mgr: &FontMgr) {
    let Some(roboto_typeface) = default_font_mgr.new_from_data(EMBEDDED_ROBOTO_REGULAR, None)
    else {
        return;
    };

    let mut provider = TypefaceFontProvider::new();
    provider.register_typeface(roboto_typeface, Some("Roboto"));
    font_collection.set_asset_font_manager(Some(provider.into()));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextFontStyle {
    Normal,
    Italic,
    Oblique,
}

impl TextFontStyle {
    fn from_int(value: i64) -> Self {
        match value {
            1 => Self::Italic,
            2 => Self::Oblique,
            _ => Self::Normal,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlignmentHorizontal {
    Left,
    Center,
    Right,
    Justify,
}

impl TextAlignmentHorizontal {
    fn from_int(value: i64) -> Self {
        match value {
            1 => Self::Center,
            2 => Self::Right,
            3 => Self::Justify,
            _ => Self::Left,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlignmentVertical {
    Top,
    Middle,
    Bottom,
}

impl TextAlignmentVertical {
    fn from_int(value: i64) -> Self {
        match value {
            1 => Self::Middle,
            2 => Self::Bottom,
            _ => Self::Top,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextAlignment {
    pub horizontal: TextAlignmentHorizontal,
    pub vertical: TextAlignmentVertical,
}

impl Default for TextAlignment {
    fn default() -> Self {
        Self {
            horizontal: TextAlignmentHorizontal::Left,
            vertical: TextAlignmentVertical::Top,
        }
    }
}

#[derive(Debug, Clone, Node)]
pub struct Text {
    pub id: NodeId,

    #[property(expected = String)]
    pub content: NodeProperty,
    #[property(expected = String)]
    pub font_family: NodeProperty,
    #[property(expected = Float)]
    pub font_size: NodeProperty,
    #[property(expected = Int)]
    pub font_weight: NodeProperty,
    #[property(expected = Int)]
    pub font_style: NodeProperty,
    #[property(expected = Float)]
    pub max_width: NodeProperty,
    #[property(expected = Color)]
    pub color: NodeProperty,
    #[property(expected = Int)]
    pub alignment_horizontal: NodeProperty,
    #[property(expected = Int)]
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
            font_style: NodeProperty::Int(0),
            max_width: NodeProperty::Float(0.0),
            color: NodeProperty::Color([255, 255, 255, 255]),
            alignment_horizontal: NodeProperty::Int(0),
            alignment_vertical: NodeProperty::Int(0),
        }
    }
}

#[node_impl]
impl Text {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<RasterFrame> {
        let content = self.resolve_content(ctx)?;
        let font_family = self.resolve_font_family(ctx)?;
        let font_size = self.resolve_font_size(ctx)? as f32;
        let font_weight = self.resolve_font_weight(ctx)? as i32;
        let font_style = TextFontStyle::from_int(self.resolve_font_style(ctx)?);
        let max_width = resolved_max_width(self.resolve_max_width(ctx)? as f32);
        let color = self.resolve_color(ctx)?;
        let alignment = TextAlignment {
            horizontal: TextAlignmentHorizontal::from_int(self.resolve_alignment_horizontal(ctx)?),
            vertical: TextAlignmentVertical::from_int(self.resolve_alignment_vertical(ctx)?),
        };

        let layout_width = max_width
            .unwrap_or(ctx.renderer.composition.render_settings.width as f32)
            .clamp(1.0, u32::MAX as f32);

        let mut paragraph_style = ParagraphStyle::new();
        paragraph_style.set_text_align(match alignment.horizontal {
            TextAlignmentHorizontal::Left => ParagraphTextAlign::Left,
            TextAlignmentHorizontal::Center => ParagraphTextAlign::Center,
            TextAlignmentHorizontal::Right => ParagraphTextAlign::Right,
            TextAlignmentHorizontal::Justify => ParagraphTextAlign::Justify,
        });

        let mut text_style = ParagraphTextStyle::new();
        text_style.set_font_size(font_size.max(1.0));
        text_style.set_color(to_skia_color(color));
        text_style.set_font_style(FontStyle::new(
            Weight::from(font_weight.clamp(100, 900)),
            skia_safe::font_style::Width::NORMAL,
            to_slant(font_style),
        ));
        let requested_font_family = font_family.trim();
        if requested_font_family.is_empty() {
            #[cfg(feature = "embed-roboto")]
            text_style.set_font_families(&["Roboto", "sans-serif"]);
            #[cfg(not(feature = "embed-roboto"))]
            text_style.set_font_families(&["sans-serif"]);
        } else {
            #[cfg(feature = "embed-roboto")]
            text_style.set_font_families(&[requested_font_family, "Roboto", "sans-serif"]);
            #[cfg(not(feature = "embed-roboto"))]
            text_style.set_font_families(&[requested_font_family, "sans-serif"]);
        }

        paragraph_style.set_text_style(&text_style);

        let font_collection = text_font_collection();
        let mut builder = ParagraphBuilder::new(&paragraph_style, font_collection);
        builder.push_style(&text_style);
        builder.add_text(&content);
        let mut paragraph = builder.build();
        paragraph.layout(layout_width);

        let rendered_width = if max_width.is_some() {
            paragraph.longest_line()
        } else {
            paragraph.max_intrinsic_width()
        }
        .max(1.0)
        .min(layout_width);

        let horizontal_offset = match alignment.horizontal {
            TextAlignmentHorizontal::Left | TextAlignmentHorizontal::Justify => 0.0,
            TextAlignmentHorizontal::Center => ((layout_width - rendered_width) * 0.5).max(0.0),
            TextAlignmentHorizontal::Right => (layout_width - rendered_width).max(0.0),
        };

        let width = rendered_width.ceil().max(1.0) as u32;
        let height = paragraph.height().ceil().max(1.0) as u32;
        let vertical_offset = match alignment.vertical {
            TextAlignmentVertical::Top => 0.0,
            TextAlignmentVertical::Middle => (height as f32 - paragraph.height()).max(0.0) * 0.5,
            TextAlignmentVertical::Bottom => (height as f32 - paragraph.height()).max(0.0),
        };
        render_to_surface_ephemeral(
            width,
            height,
            ctx,
            RectI::from_size(width, height),
            RectI::from_size(width, height),
            AlphaMode::Premultiplied,
            ClearMode::Transparent,
            |canvas| {
                paragraph.paint(canvas, (-horizontal_offset, vertical_offset));
            },
        )
    }
}

fn resolved_max_width(value: f32) -> Option<f32> {
    if value.is_finite() && value > 0.0 {
        Some(value)
    } else {
        None
    }
}

fn to_slant(style: TextFontStyle) -> skia_safe::font_style::Slant {
    match style {
        TextFontStyle::Normal => skia_safe::font_style::Slant::Upright,
        TextFontStyle::Italic => skia_safe::font_style::Slant::Italic,
        TextFontStyle::Oblique => skia_safe::font_style::Slant::Oblique,
    }
}
