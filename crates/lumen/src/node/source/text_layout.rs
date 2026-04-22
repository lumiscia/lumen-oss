use std::{
    cell::{OnceCell, RefCell},
    collections::HashMap,
};

#[cfg(feature = "embed-roboto")]
use skia_safe::textlayout::TypefaceFontProvider;
use skia_safe::{
    FontMgr, FontStyle,
    font_style::Weight,
    textlayout::{
        FontCollection, Paragraph, ParagraphBuilder, ParagraphStyle,
        TextAlign as ParagraphTextAlign, TextStyle as ParagraphTextStyle,
    },
};

use crate::node::{
    pixel_utils::to_skia_color,
    source::text::{TextAlignmentHorizontal, TextFontStyle},
};

#[cfg(feature = "embed-roboto")]
const EMBEDDED_ROBOTO_REGULAR: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/roboto/Roboto-Regular.ttf"
));

thread_local! {
    static TEXT_FONT_COLLECTION: OnceCell<FontCollection> = const { OnceCell::new() };
    static TEXT_MEASURE_CACHE: RefCell<HashMap<TextMeasureCacheKey, (f32, f32)>> = RefCell::new(HashMap::new());
}

const TEXT_MEASURE_CACHE_LIMIT: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TextMeasureCacheKey {
    text: String,
    font_family: String,
    font_size_bits: u32,
    font_weight: i32,
    font_style: TextFontStyle,
    wrap_width_bits: Option<u32>,
    fallback_width: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TextLayoutStyle {
    pub font_family: String,
    pub font_size: f32,
    pub font_weight: i32,
    pub font_style: TextFontStyle,
}

impl Default for TextLayoutStyle {
    fn default() -> Self {
        Self {
            font_family: "sans-serif".to_string(),
            font_size: 16.0,
            font_weight: 400,
            font_style: TextFontStyle::Normal,
        }
    }
}

impl TextLayoutStyle {
    pub(crate) fn new(
        font_family: impl Into<String>,
        font_size: f32,
        font_weight: i32,
        font_style: TextFontStyle,
    ) -> Self {
        Self {
            font_family: font_family.into(),
            font_size,
            font_weight,
            font_style,
        }
    }
}

pub(crate) fn resolved_max_width(value: f32) -> Option<f32> {
    if value.is_finite() && value > 0.0 {
        Some(value)
    } else {
        None
    }
}

pub(crate) fn build_paragraph(
    text: &str,
    style: &TextLayoutStyle,
    text_color: [u8; 4],
    alignment: TextAlignmentHorizontal,
) -> Paragraph {
    let mut paragraph_style = ParagraphStyle::new();
    paragraph_style.set_text_align(match alignment {
        TextAlignmentHorizontal::Left => ParagraphTextAlign::Left,
        TextAlignmentHorizontal::Center => ParagraphTextAlign::Center,
        TextAlignmentHorizontal::Right => ParagraphTextAlign::Right,
        TextAlignmentHorizontal::Justify => ParagraphTextAlign::Justify,
    });

    let mut text_style = ParagraphTextStyle::new();
    text_style.set_font_size(style.font_size.max(1.0));
    text_style.set_color(to_skia_color(text_color));
    text_style.set_font_style(FontStyle::new(
        Weight::from(style.font_weight.clamp(100, 900)),
        skia_safe::font_style::Width::NORMAL,
        to_slant(style.font_style),
    ));
    set_font_families(&mut text_style, &style.font_family);

    paragraph_style.set_text_style(&text_style);

    let font_collection = text_font_collection();
    let mut builder = ParagraphBuilder::new(&paragraph_style, font_collection);
    builder.push_style(&text_style);
    builder.add_text(text);
    builder.build()
}

pub(crate) fn measure_text(
    text: &str,
    style: &TextLayoutStyle,
    wrap_width: Option<f32>,
    fallback_width: u32,
) -> (f32, f32) {
    let key = TextMeasureCacheKey {
        text: text.to_string(),
        font_family: style.font_family.trim().to_string(),
        font_size_bits: style.font_size.to_bits(),
        font_weight: style.font_weight,
        font_style: style.font_style,
        wrap_width_bits: wrap_width.map(f32::to_bits),
        fallback_width,
    };
    if let Some(cached) = TEXT_MEASURE_CACHE.with_borrow(|cache| cache.get(&key).copied()) {
        return cached;
    }

    let layout_width = wrap_width.unwrap_or(16_384.0).clamp(1.0, u32::MAX as f32);
    let mut paragraph = build_paragraph(
        text,
        style,
        [255, 255, 255, 255],
        TextAlignmentHorizontal::Left,
    );
    paragraph.layout(layout_width);

    let width = if wrap_width.is_some() {
        paragraph.longest_line()
    } else {
        paragraph.max_intrinsic_width()
    }
    .max(1.0)
    .min(fallback_width.max(1) as f32);
    let height = paragraph.height().max(1.0);
    let measured = (width, height);
    TEXT_MEASURE_CACHE.with_borrow_mut(|cache| {
        if cache.len() >= TEXT_MEASURE_CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(key, measured);
    });
    measured
}

fn text_font_collection() -> FontCollection {
    TEXT_FONT_COLLECTION.with(|cell| {
        cell.get_or_init(|| {
            let font_mgr = FontMgr::default();
            new_text_font_collection(&font_mgr)
        })
        .clone()
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

fn set_font_families(text_style: &mut ParagraphTextStyle, font_family: &str) {
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
}

fn to_slant(style: TextFontStyle) -> skia_safe::font_style::Slant {
    match style {
        TextFontStyle::Normal => skia_safe::font_style::Slant::Upright,
        TextFontStyle::Italic => skia_safe::font_style::Slant::Italic,
        TextFontStyle::Oblique => skia_safe::font_style::Slant::Oblique,
    }
}
