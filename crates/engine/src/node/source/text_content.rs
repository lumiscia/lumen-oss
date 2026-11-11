//! Rich text params: content as a string or styled spans, the underline, strikethrough and
//! highlight decorations spans draw, the stroke along their glyphs, and OpenType font
//! features.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::expr::Expression;
use crate::node::{
    Deferred, DelegateEvalContext, DelegateEvaluable, Delegated, Override, PropertyRead,
    vector::paint::Paint,
};

use super::text::{TextFontStyle, TextTransform, text_enum};

/// Text to render, as the spans it shapes as: a string is one span that overrides nothing. In
/// JSON a string is the shorthand for unstyled text, and an `=expression` evaluates to one. No
/// spans is what the default empty string stands for.
///
/// This holds what shaping reads and nothing else, since a span's [`SpanStyle`] may measure the
/// text it belongs to.
#[derive(Debug, Clone, Default, PartialEq, lumen_macros::StructuralHash)]
pub struct TextContent(pub Vec<SpanLayout>);

impl TextContent {
    pub fn plain(text: String) -> Self {
        Self(vec![SpanLayout {
            text,
            ..SpanLayout::default()
        }])
    }
}

/// What a run of text shapes with. Absent overrides take the node's value.
#[derive(Debug, Clone, Default, PartialEq, lumen_macros::Delegate)]
pub struct SpanLayout {
    /// Text of the span. Required.
    #[meta(multiline, required)]
    pub text: String,
    /// Font family name.
    #[meta(format = "font_family")]
    pub font_family: Option<String>,
    /// Font size in pixels. Where spans of one family, weight and style meet, a ligature or
    /// other cluster across the boundary takes the size of the span it starts in.
    #[meta(min = 1, step = 1)]
    pub font_size: Option<f64>,
    /// Font weight. Variable fonts render any value in their weight range.
    #[meta(min = 1, max = 1000)]
    pub font_weight: Option<i64>,
    /// Font style.
    #[meta()]
    pub font_style: Option<TextFontStyle>,
    /// Extra space after every glyph in pixels. Negative values tighten the text.
    #[meta(step = 0.1)]
    pub letter_spacing: Option<f64>,
    /// OpenType features, merged over the node's: a feature set here wins and the others are
    /// kept. Within a word, the first span's features apply to all of it.
    #[meta()]
    pub font_features: Option<FontFeatures>,
    /// Case applied to the span's text.
    #[meta()]
    pub text_transform: Option<TextTransform>,
}

/// How a run of text draws. Absent overrides take the node's value.
#[derive(Debug, Clone, Default, PartialEq, lumen_macros::Delegate)]
pub struct SpanStyle {
    /// Text color.
    #[meta(role = "color")]
    pub color: Option<Paint>,
    /// Line under the text. `null` turns off the node's underline. Its fields take literals or
    /// expressions, but the whole line cannot be an expression.
    #[meta()]
    pub underline: Override<TextLine>,
    /// Line through the text. `null` turns off the node's strikethrough. Its fields take
    /// literals or expressions, but the whole line cannot be an expression.
    #[meta()]
    pub strikethrough: Override<TextLine>,
    /// Box behind the text. `null` turns off the node's highlight. Its fields take literals or
    /// expressions, but the whole highlight cannot be an expression.
    #[meta()]
    pub highlight: Override<TextHighlight>,
    /// Pixels the span's glyphs and its own decorations sit above the baseline; negative
    /// values sit below it. The line keeps its height, so shifted text can overlap the
    /// lines around it.
    #[meta(step = 0.5)]
    pub baseline_shift: Option<f64>,
    /// Outline along the glyphs. `null` turns off the node's stroke. Its fields take literals or
    /// expressions, but the whole stroke cannot be an expression.
    #[meta()]
    pub stroke: Override<TextStroke>,
}

/// A run of text.
#[derive(Debug, Clone, Default, PartialEq, lumen_macros::Delegate)]
pub struct TextSpan {
    #[meta(flatten)]
    pub layout: SpanLayout,
    #[meta(flatten)]
    pub style: SpanStyle,
}

/// OpenType features by four-character tag. A value of 0 turns a feature off, 1 turns it on and
/// larger values select an alternate.
#[derive(Debug, Clone, Default, PartialEq, Eq, lumen_macros::StructuralHash)]
pub struct FontFeatures(pub BTreeMap<[u8; 4], u32>);

impl FontFeatures {
    /// These features overridden by `over`'s, for shaping.
    pub(crate) fn merged(&self, over: Option<&Self>) -> Vec<lumen_text::FontFeature> {
        let mut merged = std::borrow::Cow::Borrowed(&self.0);
        if let Some(over) = over.filter(|over| !over.0.is_empty()) {
            merged.to_mut().extend(&over.0);
        }
        merged
            .iter()
            .map(|(&tag, &value)| lumen_text::FontFeature { tag, value })
            .collect()
    }
}

text_enum! {
    #[delegate(kind = "enum", expressions)]
    TextLineStyle => lumen_text::TextLineStyle {
        #[default]
        Solid = 0,
        Double = 1,
        Dotted = 2,
        Dashed = 3,
        Wavy = 4,
    }
}

/// An underline or strikethrough. Its fields take literals or expressions, but the whole line
/// cannot be an expression.
#[derive(Debug, Clone, PartialEq, lumen_macros::Delegate)]
pub struct TextLine {
    /// Line style.
    #[meta()]
    pub style: TextLineStyle,
    /// Line color. Absent uses the text color.
    #[meta(role = "color")]
    pub paint: Option<Paint>,
    /// Line thickness in pixels. Absent uses the font's.
    #[meta(min = 0, step = 0.5)]
    pub thickness: Option<f64>,
    /// Line position in pixels below the baseline; negative values are above it. Absent uses
    /// the font's underline or strikethrough position.
    #[meta(step = 0.5)]
    pub offset: Option<f64>,
    /// Start and end of the line along the text, as fractions of its width. An expression
    /// gives both as one `vec2`, not one per element.
    #[meta(step = 0.01)]
    pub extent: (f64, f64),
}

impl Default for TextLine {
    fn default() -> Self {
        Self {
            style: TextLineStyle::Solid,
            paint: None,
            thickness: None,
            offset: None,
            extent: (0.0, 1.0),
        }
    }
}

/// A box drawn behind text. Its fields take literals or expressions, but the whole highlight
/// cannot be an expression.
#[derive(Debug, Clone, PartialEq, lumen_macros::Delegate)]
pub struct TextHighlight {
    /// Box color.
    #[meta(role = "color")]
    pub paint: Paint,
    /// Horizontal and vertical padding around the text in pixels. An expression gives both as
    /// one `vec2`, not one per element.
    #[meta(step = 1)]
    pub padding: (f64, f64),
    /// Corner radius in pixels.
    #[meta(min = 0, step = 1)]
    pub radius: f64,
    /// Start and end of the box along the text, as fractions of its width. An expression
    /// gives both as one `vec2`, not one per element.
    #[meta(step = 0.01)]
    pub extent: (f64, f64),
}

impl Default for TextHighlight {
    fn default() -> Self {
        Self {
            paint: Paint::default(),
            padding: (0.0, 0.0),
            radius: 0.0,
            extent: (0.0, 1.0),
        }
    }
}

/// [`FontFeatures`] whose values take literals or expressions. The whole map cannot be an
/// expression.
#[derive(Debug, Clone, Default, lumen_macros::StructuralHash)]
pub struct FontFeaturesDelegate(pub BTreeMap<[u8; 4], Deferred<u32>>);

impl Delegated for FontFeatures {
    type Delegate = FontFeaturesDelegate;

    fn into_delegate(self) -> Self::Delegate {
        FontFeaturesDelegate(
            self.0
                .into_iter()
                .map(|(tag, value)| (tag, Deferred::Value(value)))
                .collect(),
        )
    }
}

impl DelegateEvaluable for FontFeaturesDelegate {
    type Evaluated = FontFeatures;

    const ACCEPTS_EXPRESSIONS: bool = false;

    fn eval(&self, ctx: &DelegateEvalContext<'_>) -> crate::Result<Self::Evaluated> {
        self.0
            .iter()
            .map(|(tag, value)| {
                let field = String::from_utf8_lossy(tag);
                ctx.eval_field(&field, value).map(|value| (*tag, value))
            })
            .collect::<crate::Result<_>>()
            .map(FontFeatures)
    }

    fn for_each_expression(&self, f: &mut dyn FnMut(&Expression)) {
        for value in self.0.values() {
            value.for_each_expression(f);
        }
    }

    /// Parses a JSON object of tags to non-negative integers or `=expression`s.
    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        crate::json::reject_expression(value, path)?;
        let object = value
            .as_object()
            .ok_or_else(|| format!("`{path}` expected an object of feature values, got {value}"))?;
        object
            .iter()
            .map(|(tag, value)| {
                let parsed = <[u8; 4]>::try_from(tag.as_bytes())
                    .ok()
                    .filter(|tag| tag.iter().all(|byte| (b' '..=b'~').contains(byte)))
                    .ok_or_else(|| {
                        format!(
                            "`{path}`: `{tag}` is not a feature tag, which is 4 printable ASCII characters"
                        )
                    })?;
                Deferred::from_json_value(value, &format!("{path}.{tag}"))
                    .map(|value| (parsed, value))
            })
            .collect::<Result<_, String>>()
            .map(Self)
    }

    fn to_json_value(&self) -> Option<Value> {
        Some(
            self.0
                .iter()
                .filter_map(|(tag, value)| {
                    Some((
                        String::from_utf8_lossy(tag).into_owned(),
                        value.to_json_value()?,
                    ))
                })
                .collect::<serde_json::Map<_, _>>()
                .into(),
        )
    }

    fn read(&self) -> Option<PropertyRead<'_>> {
        Some(PropertyRead::Value(None))
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum", expressions)]
pub enum TextStrokeAlign {
    /// Behind the glyphs, reaching `width` beyond their edge.
    #[default]
    Outside = 0,
    /// Over the glyphs, centred on their edge.
    Center = 1,
}

/// An outline along glyphs. Decorations are not stroked. Its fields take literals or
/// expressions, but the whole stroke cannot be an expression.
///
/// `glyph_opacity` fades a glyph's stroke and fill as one, so the fill hides the stroke under it.
/// Neighbouring glyphs fade separately, so where one's stroke overlaps another both show.
#[derive(Debug, Clone, PartialEq, lumen_macros::Delegate)]
pub struct TextStroke {
    /// Outline color.
    #[meta(role = "color")]
    pub paint: Paint,
    /// Outline width in pixels.
    #[meta(min = 0, step = 0.5)]
    pub width: f64,
    /// Where the outline lies: behind the glyphs and outside their edge, or over them and
    /// centred on it.
    #[meta()]
    pub align: TextStrokeAlign,
}

impl Default for TextStroke {
    fn default() -> Self {
        Self {
            paint: Paint::default(),
            width: 2.0,
            align: TextStrokeAlign::Outside,
        }
    }
}

impl TextStroke {
    /// How far the outline reaches to each side of the glyph edge, in pixels.
    pub(crate) fn half_width(&self) -> f64 {
        match self.align {
            TextStrokeAlign::Outside => self.width,
            TextStrokeAlign::Center => self.width * 0.5,
        }
    }
}

#[derive(Debug, Clone, lumen_macros::StructuralHash)]
pub enum TextContentDelegate {
    Text(Deferred<String>),
    Spans(Vec<TextSpanDelegate>),
}

impl Default for TextContentDelegate {
    fn default() -> Self {
        TextContent::default().into_delegate()
    }
}

impl TextContentDelegate {
    /// Moves each span's style out, leaving this holding only what shapes the text.
    pub(crate) fn take_span_styles(&mut self) -> Vec<SpanStyleDelegate> {
        match self {
            Self::Text(_) => Vec::new(),
            Self::Spans(spans) => spans
                .iter_mut()
                .map(|span| std::mem::take(&mut span.style))
                .collect(),
        }
    }

    /// The whole text when it is literal, with spans joined.
    pub fn literal_text(&self) -> Option<String> {
        match self {
            Self::Text(Deferred::Value(text)) => Some(text.clone()),
            Self::Text(Deferred::Expr(_)) => None,
            Self::Spans(spans) => spans
                .iter()
                .map(|span| match &span.layout.text {
                    Deferred::Value(text) => Some(text.as_str()),
                    Deferred::Expr(_) => None,
                })
                .collect(),
        }
    }
}

impl Delegated for TextContent {
    type Delegate = TextContentDelegate;

    fn into_delegate(self) -> Self::Delegate {
        if self.0.is_empty() {
            return TextContentDelegate::Text(String::new().into_delegate());
        }
        TextContentDelegate::Spans(
            self.0
                .into_iter()
                .map(|layout| {
                    TextSpan {
                        layout,
                        style: SpanStyle::default(),
                    }
                    .into_delegate()
                })
                .collect(),
        )
    }
}

/// Evaluates what shapes the text and leaves the spans' styles alone. A whole-content expression
/// evaluates to the text.
impl DelegateEvaluable for TextContentDelegate {
    type Evaluated = TextContent;

    const ACCEPTS_EXPRESSIONS: bool = true;

    fn eval(&self, ctx: &DelegateEvalContext<'_>) -> crate::Result<Self::Evaluated> {
        match self {
            Self::Text(text) => DelegateEvaluable::eval(text, ctx).map(TextContent::plain),
            Self::Spans(spans) => spans
                .iter()
                .enumerate()
                .map(|(index, span)| span.layout.eval(ctx).map_err(|error| error.in_index(index)))
                .collect::<crate::Result<_>>()
                .map(TextContent),
        }
    }

    fn for_each_expression(&self, f: &mut dyn FnMut(&Expression)) {
        match self {
            Self::Text(text) => text.for_each_expression(f),
            Self::Spans(spans) => spans.for_each_expression(f),
        }
    }

    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        if value.is_array() {
            DelegateEvaluable::from_json_value(value, path).map(Self::Spans)
        } else {
            DelegateEvaluable::from_json_value(value, path).map(Self::Text)
        }
    }

    fn to_json_value(&self) -> Option<Value> {
        match self {
            Self::Text(text) => text.to_json_value(),
            Self::Spans(spans) => spans.to_json_value(),
        }
    }

    fn read(&self) -> Option<PropertyRead<'_>> {
        match self {
            Self::Text(text) => text.read(),
            Self::Spans(_) => Some(PropertyRead::Value(
                self.literal_text()
                    .map(crate::expr::ExpressionValue::String),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        expr::ExpressionContext,
        node::{NodeId, NodeParams},
    };

    fn parse(value: serde_json::Value) -> Result<TextContentDelegate, String> {
        TextContentDelegate::from_json_value(&value, "content")
    }

    fn eval(content: &TextContentDelegate) -> crate::Result<TextContent> {
        let expr = ExpressionContext {
            frame: 12.0,
            ..ExpressionContext::default()
        };
        DelegateEvalContext {
            node_id: NodeId::new(3),
            expr: &expr,
        }
        .eval_field("content", content)
    }

    /// The layout and style of each span of `content`.
    fn eval_spans(content: &TextContentDelegate) -> crate::Result<Vec<TextSpan>> {
        let TextContentDelegate::Spans(spans) = content else {
            panic!("expected spans");
        };
        let expr = ExpressionContext {
            frame: 12.0,
            ..ExpressionContext::default()
        };
        DelegateEvalContext {
            node_id: NodeId::new(3),
            expr: &expr,
        }
        .eval_field("content", spans)
    }

    #[test]
    fn plain_strings_and_expressions_are_text() {
        let content = parse(serde_json::json!("Hello")).unwrap();
        assert_eq!(eval(&content).unwrap(), TextContent::plain("Hello".into()));
        let content = parse(serde_json::json!("=frame * 2")).unwrap();
        assert_eq!(eval(&content).unwrap(), TextContent::plain("24".into()));
        let content = parse(serde_json::json!("==sum")).unwrap();
        assert_eq!(eval(&content).unwrap(), TextContent::plain("=sum".into()));
    }

    #[test]
    fn content_and_span_text_build_a_readout_from_the_frame() {
        let expr = ExpressionContext {
            frame: 30.0,
            ..ExpressionContext::default()
        };
        let ctx = DelegateEvalContext {
            node_id: NodeId::new(3),
            expr: &expr,
        };
        let readout = r#"=concat("frame ", format(frame, 0, 4, "0"), " / 1200")"#;
        let content = parse(serde_json::json!(readout)).unwrap();
        assert_eq!(
            ctx.eval_field("content", &content).unwrap(),
            TextContent::plain("frame 0030 / 1200".into())
        );
        let content = parse(serde_json::json!([
            { "text": "t = " },
            { "text": "=format(frame / 7, 2)" },
            { "text": "=round(frame / 7)" }
        ]))
        .unwrap();
        let spans = ctx.eval_field("content", &content).unwrap();
        let texts: Vec<_> = spans.0.iter().map(|span| span.text.as_str()).collect();
        assert_eq!(texts, ["t = ", "4.29", "4"]);
    }

    #[test]
    fn the_default_is_the_empty_string() {
        let content = TextContent::default().into_delegate();
        assert_eq!(content.to_json_value(), Some(serde_json::json!("")));
        assert_eq!(eval(&content).unwrap(), TextContent::plain(String::new()));
    }

    #[test]
    fn evaluating_content_leaves_span_styles_alone() {
        let mut content = parse(serde_json::json!([{
            "text": "=frame",
            "color": "=node(99, 'color')",
            "baseline_shift": "=node(99, 'x')",
            "underline": { "offset": "=node(99, 'x')" }
        }]))
        .unwrap();
        assert_eq!(eval(&content).unwrap(), TextContent::plain("12".into()));

        let styles = content.take_span_styles();
        assert_eq!(styles.len(), 1);
        assert!(styles[0].has_expressions());
        assert!(eval(&content).is_ok());
        assert_eq!(
            content.to_json_value().unwrap(),
            serde_json::json!([{ "text": "=frame" }])
        );
    }

    #[test]
    fn spans_parse_overrides_expressions_and_cleared_decorations() {
        let content = parse(serde_json::json!([
            { "text": "Hello, " },
            {
                "text": "world",
                "font_weight": "=frame * 50",
                "font_style": "italic",
                "text_transform": "uppercase",
                "baseline_shift": "=frame / 2",
                "color": "#ff0000",
                "underline": { "style": "wavy", "thickness": 2, "extent": "=vec2(0, frame / 24)" },
                "strikethrough": null,
                "highlight": { "paint": [255, 255, 0], "padding": [4, 2], "radius": 3 }
            }
        ]))
        .unwrap();
        let spans = eval_spans(&content).unwrap();
        assert_eq!(
            spans[0],
            TextSpan {
                layout: SpanLayout {
                    text: "Hello, ".into(),
                    ..SpanLayout::default()
                },
                ..TextSpan::default()
            }
        );
        assert_eq!(spans[1].layout.font_weight, Some(600));
        assert_eq!(spans[1].layout.font_style, Some(TextFontStyle::Italic));
        assert_eq!(
            spans[1].layout.text_transform,
            Some(TextTransform::Uppercase)
        );
        assert_eq!(spans[1].style.baseline_shift, Some(6.0));
        assert_eq!(spans[1].style.color, Some(Paint::solid([255, 0, 0, 255])));
        assert_eq!(
            spans[1].style.underline,
            Override::Set(TextLine {
                style: TextLineStyle::Wavy,
                thickness: Some(2.0),
                extent: (0.0, 0.5),
                ..TextLine::default()
            })
        );
        assert_eq!(spans[1].style.strikethrough, Override::Clear);
        assert_eq!(
            spans[1].style.highlight,
            Override::Set(TextHighlight {
                paint: Paint::solid([255, 255, 0, 255]),
                padding: (4.0, 2.0),
                radius: 3.0,
                extent: (0.0, 1.0),
            })
        );
    }

    #[test]
    fn json_round_trips_through_the_delegate() {
        let json = serde_json::json!([
            { "text": "==a", "font_size": "=frame", "underline": {}, "highlight": null },
            { "text": "b", "strikethrough": { "style": "dashed", "paint": "=#00ff00" } }
        ]);
        let content = parse(json).unwrap();
        assert_eq!(
            content.to_json_value().unwrap(),
            serde_json::json!([
                {
                    "text": "==a",
                    "font_size": "=frame",
                    "underline": { "style": "solid", "extent": [0.0, 1.0] },
                    "highlight": null
                },
                {
                    "text": "b",
                    "strikethrough": { "style": "dashed", "paint": "=#00ff00", "extent": [0.0, 1.0] }
                }
            ])
        );
        assert_eq!(
            TextLineDelegate::default().to_json_value().unwrap(),
            serde_json::json!({ "style": "solid", "extent": [0.0, 1.0] })
        );
    }

    #[test]
    fn errors_name_the_span_field() {
        for (json, expected) in [
            (
                serde_json::json!([{ "text": "a", "font_size": "big" }]),
                "`content[0].font_size` expected float",
            ),
            (
                serde_json::json!([{ "text": "a" }, { "text": "b", "font_wieght": 700 }]),
                "`content[1]`: unknown field `font_wieght`",
            ),
            (
                serde_json::json!([{ "text": "a", "font_size": null }]),
                "`content[0].font_size` expected float",
            ),
            (
                serde_json::json!([{ "text": "a" }, { "font_size": 12 }]),
                "`content[1]`: missing field `text`",
            ),
            (
                serde_json::json!([{ "text": "a", "font_style": "bold" }]),
                "`content[0].font_style`",
            ),
            (
                serde_json::json!([{ "text": "a", "underline": "=1" }]),
                "`content[0].underline` does not support expressions",
            ),
        ] {
            let error = parse(json.clone()).unwrap_err();
            assert!(error.contains(expected), "{json}: {error}");
        }
    }

    #[test]
    fn enum_fields_take_expressions() {
        let content = parse(serde_json::json!([{
            "text": "a",
            "font_style": "=frame - 11",
            "underline": { "style": "=frame / 12" }
        }]))
        .unwrap();
        let spans = eval_spans(&content).unwrap();
        assert_eq!(spans[0].layout.font_style, Some(TextFontStyle::Italic));
        assert_eq!(
            spans[0].style.underline,
            Override::Set(TextLine {
                style: TextLineStyle::Double,
                ..TextLine::default()
            })
        );

        let content = parse(serde_json::json!([{
            "text": "a",
            "font_style": "=if(frame > 10, 'oblique', 'normal')",
            "underline": { "style": "='dashed'" }
        }]))
        .unwrap();
        let spans = eval_spans(&content).unwrap();
        assert_eq!(spans[0].layout.font_style, Some(TextFontStyle::Oblique));
        assert_eq!(
            spans[0].style.underline,
            Override::Set(TextLine {
                style: TextLineStyle::Dashed,
                ..TextLine::default()
            })
        );

        let content = parse(serde_json::json!([{ "text": "a", "font_style": "='bold'" }])).unwrap();
        let error = eval(&content).unwrap_err().to_string();
        assert!(
            error.contains("one of `normal`, `italic`, `oblique` or a number")
                && error.contains("`bold`"),
            "{error}"
        );
    }

    #[test]
    fn expression_errors_carry_the_span_field_path() {
        let content = parse(serde_json::json!([
            { "text": "a" },
            {
                "text": "b",
                "font_size": "=vec2(1, vec2(2, 3))",
                "underline": { "extent": "=vec2(1, vec2(2, 3))" }
            }
        ]))
        .unwrap();
        assert_eq!(
            eval(&content).unwrap_err().to_string(),
            "node 3 `content[1].font_size`: vec2 expects numeric arguments"
        );

        let content = parse(serde_json::json!([
            { "text": "a", "underline": { "extent": "=vec2(1, vec2(2, 3))" } }
        ]))
        .unwrap();
        assert_eq!(
            eval_spans(&content).unwrap_err().to_string(),
            "node 3 `content[0].underline.extent`: vec2 expects numeric arguments"
        );

        let content =
            parse(serde_json::json!([{ "text": "a", "font_size": "=\"large\"" }])).unwrap();
        assert_eq!(
            eval(&content).unwrap_err().to_string(),
            "node 3 `content[0].font_size`: expected a number, got string `large`"
        );

        let error =
            parse(serde_json::json!([{ "text": "a", "font_size": "=foo + 1" }])).unwrap_err();
        assert!(error.contains("font_size"), "{error}");
        assert!(error.contains("unknown variable `foo`"), "{error}");
    }

    #[test]
    fn font_features_check_tags_and_values_and_merge_over_the_node() {
        let features =
            |json: serde_json::Value| FontFeaturesDelegate::from_json_value(&json, "font_features");
        for (json, expected) in [
            (
                serde_json::json!({ "tnu": 1 }),
                "`tnu` is not a feature tag",
            ),
            (
                serde_json::json!({ "tnum1": 1 }),
                "`tnum1` is not a feature tag",
            ),
            (
                serde_json::json!({ "tnüm": 1 }),
                "`tnüm` is not a feature tag",
            ),
            (
                serde_json::json!({ "tnum": -1 }),
                "`font_features.tnum` must be an integer from 0 to 4294967295",
            ),
            (serde_json::json!({ "tnum": 1.5 }), "whole number"),
            (serde_json::json!([1]), "expected an object"),
        ] {
            let error = features(json.clone()).unwrap_err();
            assert!(error.contains(expected), "{json}: {error}");
        }

        let node = features(serde_json::json!({ "tnum": 1, "liga": 1, "ss01": 2 }))
            .unwrap()
            .eval(&DelegateEvalContext {
                node_id: NodeId::new(3),
                expr: &ExpressionContext::default(),
            })
            .unwrap();
        let content = parse(serde_json::json!([
            { "text": "a" },
            { "text": "b", "font_features": { "liga": 0, "onum": "=frame - 11", "ss01": "=frame / 6" } }
        ]))
        .unwrap();
        let TextContent(spans) = eval(&content).unwrap();
        let merged = |over: Option<&FontFeatures>| {
            node.merged(over)
                .into_iter()
                .map(|feature| {
                    (
                        String::from_utf8(feature.tag.to_vec()).unwrap(),
                        feature.value,
                    )
                })
                .collect::<Vec<_>>()
        };
        let tags = |features: &[(&str, u32)]| {
            features
                .iter()
                .map(|(tag, value)| (tag.to_string(), *value))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            merged(spans[0].font_features.as_ref()),
            tags(&[("liga", 1), ("ss01", 2), ("tnum", 1)])
        );
        assert_eq!(
            merged(spans[1].font_features.as_ref()),
            tags(&[("liga", 0), ("onum", 1), ("ss01", 2), ("tnum", 1)])
        );

        for value in ["=vec2(1, 2)", "=0 - frame"] {
            let content =
                parse(serde_json::json!([{ "text": "a", "font_features": { "tnum": value } }]))
                    .unwrap();
            let error = eval(&content).unwrap_err().to_string();
            assert!(
                error.starts_with("node 3 `content[0].font_features.tnum`: "),
                "{value}: {error}"
            );
        }
    }

    #[test]
    fn strings_through_the_property_parser_are_classified_once() {
        #[derive(Debug, Clone, Default, lumen_macros::Delegate)]
        pub struct Probe {
            #[meta()]
            pub content: TextContent,
        }

        let content = |value: serde_json::Value| {
            let params = serde_json::json!({ "content": value });
            let probe =
                <ProbeDelegate as NodeParams>::from_json(NodeId::new(3), params.as_object())
                    .unwrap();
            let expr = ExpressionContext {
                frame: 12.0,
                ..ExpressionContext::default()
            };
            DelegateEvaluable::eval(
                &probe,
                &crate::node::DelegateEvalContext {
                    node_id: NodeId::new(3),
                    expr: &expr,
                },
            )
            .map(|probe| probe.content)
        };
        for (json, expected) in [
            ("Hello", "Hello"),
            ("=frame", "12"),
            ("==frame", "=frame"),
            ("==", "="),
            ("===", "=="),
        ] {
            assert_eq!(
                content(serde_json::json!(json)).unwrap(),
                TextContent::plain(expected.into()),
                "{json}"
            );
        }
    }
}
