//! Rich text params: content as a string or styled spans, the underline, strikethrough and
//! highlight decorations spans draw, the stroke along their glyphs, and OpenType font
//! features.

use std::collections::BTreeMap;

use crate::node::{
    Deferred, DelegateEvalContext, DelegateEvaluable, DelegateValue, DelegateVec, Delegated,
    NodeId, Override, PropertyExpression, PropertyValue, vector::paint::Paint,
};

use super::text::{TextFontStyle, TextTransform};

/// Text to render: a string, or spans that each override the node's text style. In JSON a
/// string is the shorthand for unstyled text, and an `=expression` evaluates to one.
#[derive(Debug, Clone, PartialEq, lumen_macros::StructuralHash)]
pub enum TextContent {
    Text(String),
    Spans(Vec<TextSpan>),
}

impl Default for TextContent {
    fn default() -> Self {
        Self::Text(String::new())
    }
}

/// A run of text. Absent overrides take the node's value.
#[derive(Debug, Clone, Default, PartialEq, lumen_macros::Delegate)]
#[delegate(properties)]
pub struct TextSpan {
    /// Text of the span. Required.
    #[meta(multiline, required)]
    pub text: String,
    /// Font family name.
    #[meta(format = "font_family")]
    pub font_family: Option<String>,
    /// Font size in pixels.
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
    /// Case applied to the span's text.
    #[meta()]
    pub text_transform: Option<TextTransform>,
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

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum", expressions)]
pub enum TextLineStyle {
    #[default]
    Solid = 0,
    Double = 1,
    Dotted = 2,
    Dashed = 3,
    Wavy = 4,
}

/// An underline or strikethrough. Its fields take literals or expressions, but the whole line
/// cannot be an expression.
#[derive(Debug, Clone, PartialEq, lumen_macros::Delegate)]
#[delegate(properties)]
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
#[delegate(properties)]
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

    fn has_expressions(&self) -> bool {
        self.0.values().any(Deferred::is_expr)
    }
}

impl FontFeaturesDelegate {
    /// Parses a JSON object of tags to non-negative integers or `=expression`s.
    pub fn from_json_value(value: &serde_json::Value) -> Result<Self, String> {
        let object = value
            .as_object()
            .ok_or_else(|| format!("expected an object of feature values, got {value}"))?;
        object
            .iter()
            .map(|(tag, value)| {
                let parsed = <[u8; 4]>::try_from(tag.as_bytes())
                    .ok()
                    .filter(|tag| tag.iter().all(|byte| (b' '..=b'~').contains(byte)))
                    .ok_or_else(|| {
                        format!(
                            "`{tag}` is not a feature tag, which is 4 printable ASCII characters"
                        )
                    })?;
                let value = match serde_json::from_value::<Deferred<i64>>(value.clone())
                    .map_err(|error| format!("`{tag}`: {error}"))?
                {
                    Deferred::Value(number) => {
                        u32::try_from(number).map(Deferred::Value).map_err(|_| {
                            format!("`{tag}` must be a non-negative integer, got {number}")
                        })?
                    }
                    Deferred::Expr(expr) => Deferred::Expr(expr),
                };
                Ok((parsed, value))
            })
            .collect::<Result<_, String>>()
            .map(Self)
    }

    pub fn to_json_value(&self) -> serde_json::Value {
        self.0
            .iter()
            .map(|(tag, value)| {
                (
                    String::from_utf8_lossy(tag).into_owned(),
                    crate::json::property_to_json(&value.to_property_expression(), None),
                )
            })
            .collect::<serde_json::Map<_, _>>()
            .into()
    }
}

impl<'de> serde::Deserialize<'de> for FontFeaturesDelegate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <serde_json::Value as serde::Deserialize>::deserialize(deserializer)?;
        Self::from_json_value(&value).map_err(serde::de::Error::custom)
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
#[delegate(properties)]
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
    Spans(DelegateVec<TextSpan>),
}

impl Default for TextContentDelegate {
    fn default() -> Self {
        TextContent::default().into_delegate()
    }
}

impl TextContentDelegate {
    /// Parses `value` as written in JSON, where a string starting with `=` is an expression.
    pub fn from_json_value(value: &serde_json::Value) -> Result<Self, String> {
        if value.is_array() {
            crate::json::nested_from_json(value).map(Self::Spans)
        } else {
            crate::json::nested_from_json(value).map(Self::Text)
        }
    }

    pub fn to_json_value(&self) -> serde_json::Value {
        match self {
            Self::Text(text) => crate::json::property_to_json(&text.to_property_expression(), None),
            Self::Spans(spans) => spans
                .items
                .iter()
                .map(|span| serde_json::Value::Object(span.to_json()))
                .collect(),
        }
    }
}

impl<'de> serde::Deserialize<'de> for TextContentDelegate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <serde_json::Value as serde::Deserialize>::deserialize(deserializer)?;
        Self::from_json_value(&value).map_err(serde::de::Error::custom)
    }
}

impl TextContentDelegate {
    /// Evaluates the text and what shapes it, leaving the spans' colors, decorations and baseline
    /// shifts, which a measurement of the text may be an input of, unevaluated.
    pub(crate) fn eval_layout(&self, ctx: &DelegateEvalContext<'_>) -> crate::Result<TextContent> {
        let Self::Spans(spans) = self else {
            return self.eval(ctx);
        };
        spans
            .items
            .iter()
            .enumerate()
            .map(|(index, span)| {
                TextSpanDelegate {
                    text: span.text.clone(),
                    font_family: span.font_family.clone(),
                    font_size: span.font_size.clone(),
                    font_weight: span.font_weight.clone(),
                    font_style: span.font_style.clone(),
                    letter_spacing: span.letter_spacing.clone(),
                    font_features: span.font_features.clone(),
                    text_transform: span.text_transform.clone(),
                    baseline_shift: None,
                    color: None,
                    underline: Override::Inherit,
                    strikethrough: Override::Inherit,
                    highlight: Override::Inherit,
                    stroke: Override::Inherit,
                }
                .eval(ctx)
                .map_err(|error| error.in_index(index))
            })
            .collect::<crate::Result<_>>()
            .map(TextContent::Spans)
    }

    /// The whole text when it is literal, with spans joined.
    pub fn literal_text(&self) -> Option<String> {
        match self {
            Self::Text(Deferred::Value(text)) => Some(text.clone()),
            Self::Text(Deferred::Expr(_)) => None,
            Self::Spans(spans) => spans
                .items
                .iter()
                .map(|span| match &span.text {
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
        match self {
            Self::Text(text) => TextContentDelegate::Text(text.into_delegate()),
            Self::Spans(spans) => TextContentDelegate::Spans(spans.into_delegate()),
        }
    }
}

impl DelegateEvaluable for TextContentDelegate {
    type Evaluated = TextContent;

    fn eval(&self, ctx: &DelegateEvalContext<'_>) -> crate::Result<Self::Evaluated> {
        Ok(match self {
            Self::Text(text) => TextContent::Text(DelegateEvaluable::eval(text, ctx)?),
            Self::Spans(spans) => TextContent::Spans(spans.eval(ctx)?),
        })
    }

    fn has_expressions(&self) -> bool {
        match self {
            Self::Text(text) => text.has_expressions(),
            Self::Spans(spans) => spans.has_expressions(),
        }
    }
}

/// A whole-content expression evaluates to the text.
impl DelegateValue for TextContentDelegate {
    const ACCEPTS_EXPRESSIONS: bool = true;

    fn to_property_value(&self) -> PropertyValue {
        PropertyValue::TextContent(Box::new(self.clone()))
    }

    fn to_property_expression(&self) -> PropertyExpression {
        match self {
            Self::Text(Deferred::Expr(expr)) => PropertyExpression::Expr(expr.clone()),
            _ => PropertyExpression::Value(self.to_property_value()),
        }
    }

    fn from_property_expression(
        value: PropertyExpression,
        node_id: NodeId,
        property_path: &str,
    ) -> crate::Result<Self> {
        match value {
            PropertyExpression::Value(PropertyValue::TextContent(content)) => Ok(*content),
            PropertyExpression::Value(PropertyValue::String(text)) => {
                Ok(Self::Text(Deferred::Value(text)))
            }
            PropertyExpression::Expr(expr) => Ok(Self::Text(Deferred::Expr(expr))),
            PropertyExpression::Value(_) => Err(PropertyValue::invalid_type(
                node_id,
                property_path,
                "TextContent",
                "incompatible value",
            )),
        }
    }
}

macro_rules! impl_struct_kind_value {
    ($delegate:ty, $variant:ident) => {
        impl DelegateValue for $delegate {
            const ACCEPTS_EXPRESSIONS: bool = false;

            fn to_property_value(&self) -> PropertyValue {
                PropertyValue::$variant(Box::new(self.clone()))
            }

            fn from_property_expression(
                value: PropertyExpression,
                node_id: NodeId,
                property_path: &str,
            ) -> crate::Result<Self> {
                match value {
                    PropertyExpression::Value(PropertyValue::$variant(value)) => Ok(*value),
                    PropertyExpression::Value(_) => Err(PropertyValue::invalid_type(
                        node_id,
                        property_path,
                        stringify!($variant),
                        "incompatible value",
                    )),
                    PropertyExpression::Expr(_) => Err(PropertyValue::invalid_type(
                        node_id,
                        property_path,
                        stringify!($variant),
                        "expression",
                    )),
                }
            }
        }
    };
}

impl_struct_kind_value!(TextLineDelegate, TextLine);
impl_struct_kind_value!(TextHighlightDelegate, TextHighlight);
impl_struct_kind_value!(FontFeaturesDelegate, FontFeatures);
impl_struct_kind_value!(TextStrokeDelegate, TextStroke);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{expr::ExpressionContext, node::NodeParams};

    fn parse(value: serde_json::Value) -> Result<TextContentDelegate, String> {
        TextContentDelegate::from_json_value(&value)
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

    #[test]
    fn plain_strings_and_expressions_are_text() {
        let content = parse(serde_json::json!("Hello")).unwrap();
        assert_eq!(eval(&content).unwrap(), TextContent::Text("Hello".into()));
        let content = parse(serde_json::json!("=frame * 2")).unwrap();
        assert_eq!(eval(&content).unwrap(), TextContent::Text("24".into()));
        let content = parse(serde_json::json!("==sum")).unwrap();
        assert_eq!(eval(&content).unwrap(), TextContent::Text("=sum".into()));
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
        let TextContent::Spans(spans) = eval(&content).unwrap() else {
            panic!("expected spans");
        };
        assert_eq!(
            spans[0],
            TextSpan {
                text: "Hello, ".into(),
                ..TextSpan::default()
            }
        );
        assert_eq!(spans[1].font_weight, Some(600));
        assert_eq!(spans[1].font_style, Some(TextFontStyle::Italic));
        assert_eq!(spans[1].text_transform, Some(TextTransform::Uppercase));
        assert_eq!(spans[1].baseline_shift, Some(6.0));
        assert_eq!(spans[1].color, Some(Paint::solid([255, 0, 0, 255])));
        assert_eq!(
            spans[1].underline,
            Override::Set(TextLine {
                style: TextLineStyle::Wavy,
                thickness: Some(2.0),
                extent: (0.0, 0.5),
                ..TextLine::default()
            })
        );
        assert_eq!(spans[1].strikethrough, Override::Clear);
        assert_eq!(
            spans[1].highlight,
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
            content.to_json_value(),
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
            TextLineDelegate::default().to_json(),
            serde_json::json!({ "style": "solid", "extent": [0.0, 1.0] })
                .as_object()
                .unwrap()
                .clone()
        );
    }

    #[test]
    fn errors_name_the_span_field() {
        for (json, expected) in [
            (
                serde_json::json!([{ "text": "a", "font_size": "big" }]),
                "[0]: `font_size` expected float",
            ),
            (
                serde_json::json!([{ "text": "a" }, { "text": "b", "font_wieght": 700 }]),
                "[1]: unknown field `font_wieght`",
            ),
            (
                serde_json::json!([{ "text": "a", "font_size": null }]),
                "[0]: `font_size` expected float",
            ),
            (
                serde_json::json!([{ "text": "a" }, { "font_size": 12 }]),
                "[1]: missing field `text`",
            ),
            (
                serde_json::json!([{ "text": "a", "font_style": "bold" }]),
                "[0]: `font_style`",
            ),
            (
                serde_json::json!([{ "text": "a", "underline": "=1" }]),
                "[0]: `underline` expected TextLine, got expression",
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
        let TextContent::Spans(spans) = eval(&content).unwrap() else {
            panic!("expected spans");
        };
        assert_eq!(spans[0].font_style, Some(TextFontStyle::Italic));
        assert_eq!(
            spans[0].underline,
            Override::Set(TextLine {
                style: TextLineStyle::Double,
                ..TextLine::default()
            })
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
            eval(&content).unwrap_err().to_string(),
            "node 3 `content[0].underline.extent`: vec2 expects numeric arguments"
        );

        let content =
            parse(serde_json::json!([{ "text": "a", "font_size": "=\"large\"" }])).unwrap();
        assert_eq!(
            eval(&content).unwrap_err().to_string(),
            "node 3 `content[0].font_size`: expected a number, got string `large`"
        );

        let content = parse(serde_json::json!([{ "text": "a", "font_size": "=foo + 1" }])).unwrap();
        assert_eq!(
            eval(&content).unwrap_err().to_string(),
            "node 3 `content[0].font_size`: undefined variable `foo`"
        );
    }

    #[test]
    fn font_features_check_tags_and_values_and_merge_over_the_node() {
        let features = |json: serde_json::Value| FontFeaturesDelegate::from_json_value(&json);
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
                "`tnum` must be a non-negative integer",
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
        let TextContent::Spans(spans) = eval(&content).unwrap() else {
            panic!("expected spans");
        };
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
        #[delegate(properties)]
        pub struct Probe {
            #[meta()]
            pub content: TextContent,
        }

        let content = |value: serde_json::Value| {
            let params = serde_json::json!({ "content": value });
            let probe = ProbeDelegate::from_json(NodeId::new(3), params.as_object()).unwrap();
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
                TextContent::Text(expected.into()),
                "{json}"
            );
        }
    }
}
