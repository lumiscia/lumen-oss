use std::{cell::RefCell, sync::Arc};

#[cfg(feature = "embed-roboto")]
use skia_safe::textlayout::TypefaceFontProvider;
use skia_safe::{
    Color, FontMgr, FontStyle,
    font_style::Weight,
    surfaces,
    textlayout::{
        FontCollection, ParagraphBuilder, ParagraphStyle, TextAlign as ParagraphTextAlign,
        TextStyle as ParagraphTextStyle,
    },
};

use crate::{
    error::LumenError,
    node::{
        InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue,
        pixel_utils::{read_surface_rgba, to_skia_color},
    },
    raster::RasterFrame,
    render::RenderContext,
};

thread_local! {
    static TEXT_FONT_MGR: RefCell<Option<FontMgr>> = const { RefCell::new(None) };
    static TEXT_FONT_COLLECTION: RefCell<Option<FontCollection>> = const { RefCell::new(None) };
}

#[cfg(feature = "embed-roboto")]
const EMBEDDED_ROBOTO_REGULAR: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/roboto/Roboto-Regular.ttf"
));

fn with_text_font_mgr<R>(f: impl FnOnce(&FontMgr) -> R) -> R {
    TEXT_FONT_MGR.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let mgr = borrow.get_or_insert_with(FontMgr::default);
        f(mgr)
    })
}

fn with_text_font_collection<R>(f: impl FnOnce(FontCollection) -> R) -> R {
    TEXT_FONT_COLLECTION.with(|cell| {
        if cell.borrow().is_none() {
            let font_collection = with_text_font_mgr(new_text_font_collection);
            *cell.borrow_mut() = Some(font_collection);
        }

        let font_collection = cell
            .borrow()
            .as_ref()
            .expect("text font collection should be initialized")
            .clone();

        f(font_collection)
    })
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlignmentHorizontal {
    Left,
    Center,
    Right,
    Justify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlignmentVertical {
    Top,
    Middle,
    Bottom,
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

#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    pub content: String,
    pub font_family: String,
    pub font_size: f32,
    pub font_weight: u16,
    pub font_style: TextFontStyle,
    pub max_width: Option<f32>,
    pub color: [u8; 4],
    pub alignment: TextAlignment,
}

impl Default for Text {
    fn default() -> Self {
        Self {
            content: String::new(),
            font_family: "sans-serif".to_string(),
            font_size: 16.0,
            font_weight: 400,
            font_style: TextFontStyle::Normal,
            max_width: None,
            color: [255, 255, 255, 255],
            alignment: TextAlignment::default(),
        }
    }
}

impl NodeEval for Text {
    fn input_port_defs(&self) -> &'static [InputPortDef] {
        &[]
    }

    fn output_port_defs(&self) -> &'static [OutputPortDef] {
        &[OutputPortDef {
            name: "output",
            kind: PortKind::RasterFrame,
        }]
    }

    fn evaluate(
        &self,
        _inputs: &NodeInputs,
        ctx: &mut RenderContext,
    ) -> Result<PortValue, LumenError> {
        let layout_width = self
            .max_width
            .unwrap_or(ctx.request.width() as f32)
            .clamp(1.0, u32::MAX as f32);

        let mut paragraph_style = ParagraphStyle::new();
        paragraph_style.set_text_align(match self.alignment.horizontal {
            TextAlignmentHorizontal::Left => ParagraphTextAlign::Left,
            TextAlignmentHorizontal::Center => ParagraphTextAlign::Center,
            TextAlignmentHorizontal::Right => ParagraphTextAlign::Right,
            TextAlignmentHorizontal::Justify => ParagraphTextAlign::Justify,
        });

        let mut text_style = ParagraphTextStyle::new();
        text_style.set_font_size(self.font_size.max(1.0));
        text_style.set_color(to_skia_color(self.color));
        text_style.set_font_style(FontStyle::new(
            Weight::from(i32::from(self.font_weight.clamp(100, 900))),
            skia_safe::font_style::Width::NORMAL,
            to_slant(self.font_style),
        ));
        let requested_font_family = self.font_family.trim();
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

        let paragraph = with_text_font_collection(|font_collection| {
            let mut builder = ParagraphBuilder::new(&paragraph_style, font_collection);
            builder.push_style(&text_style);
            builder.add_text(&self.content);
            let mut paragraph = builder.build();
            paragraph.layout(layout_width);
            paragraph
        });

        let width = layout_width.ceil().max(1.0) as u32;
        let height = paragraph.height().ceil().max(1.0) as u32;
        let Some(mut surface) = surfaces::raster_n32_premul((width as i32, height as i32)) else {
            return Ok(PortValue::RasterFrame(RasterFrame::bitmap(
                Arc::new(vec![0_u8; 4]),
                1,
                1,
            )));
        };

        let canvas = surface.canvas();
        canvas.clear(Color::TRANSPARENT);
        let vertical_offset = match self.alignment.vertical {
            TextAlignmentVertical::Top => 0.0,
            TextAlignmentVertical::Middle => (height as f32 - paragraph.height()).max(0.0) * 0.5,
            TextAlignmentVertical::Bottom => (height as f32 - paragraph.height()).max(0.0),
        };
        paragraph.paint(canvas, (0.0, vertical_offset));

        let bytes = read_surface_rgba(&mut surface, width, height, Some(ctx));
        Ok(PortValue::RasterFrame(RasterFrame::bitmap(
            Arc::new(bytes),
            width,
            height,
        )))
    }
}

fn to_slant(style: TextFontStyle) -> skia_safe::font_style::Slant {
    match style {
        TextFontStyle::Normal => skia_safe::font_style::Slant::Upright,
        TextFontStyle::Italic => skia_safe::font_style::Slant::Italic,
        TextFontStyle::Oblique => skia_safe::font_style::Slant::Oblique,
    }
}
