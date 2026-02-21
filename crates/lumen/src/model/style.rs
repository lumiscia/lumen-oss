use serde::{Deserialize, Serialize};

pub type ColorRgba = [u8; 4];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum StyleValue {
    Value(f32),
    Expr(String),
}

impl StyleValue {
    pub fn as_literal(&self) -> Option<f32> {
        match self {
            Self::Value(value) => Some(*value),
            Self::Expr(_) => None,
        }
    }

    pub fn as_expr(&self) -> Option<&str> {
        match self {
            Self::Value(_) => None,
            Self::Expr(expr) => Some(expr.as_str()),
        }
    }
}

impl Default for StyleValue {
    fn default() -> Self {
        Self::Value(0.0)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl Default for BlendMode {
    fn default() -> Self {
        Self::Normal
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct BaseStyle {
    #[serde(default = "default_true")]
    pub visible: bool,
    #[serde(default = "default_opacity")]
    pub opacity: StyleValue,
    #[serde(default)]
    pub blend_mode: BlendMode,
    #[serde(default)]
    pub blur: StyleValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow: Option<ShadowStyle>,
    #[serde(default)]
    pub transform: TransformStyle,
    #[serde(default = "default_alignment")]
    pub alignment: [StyleValue; 2],
}

impl Default for BaseStyle {
    fn default() -> Self {
        Self {
            visible: default_true(),
            opacity: default_opacity(),
            blend_mode: BlendMode::Normal,
            blur: StyleValue::Value(0.0),
            shadow: None,
            transform: TransformStyle::default(),
            alignment: default_alignment(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ShadowStyle {
    #[serde(default)]
    pub offset_x: StyleValue,
    #[serde(default)]
    pub offset_y: StyleValue,
    #[serde(default = "default_shadow_blur")]
    pub blur: StyleValue,
    #[serde(default = "default_shadow_color")]
    pub color: [u8; 4],
}

impl Default for ShadowStyle {
    fn default() -> Self {
        Self {
            offset_x: StyleValue::Value(4.0),
            offset_y: StyleValue::Value(4.0),
            blur: default_shadow_blur(),
            color: default_shadow_color(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct TransformStyle {
    #[serde(default)]
    pub x: StyleValue,
    #[serde(default)]
    pub y: StyleValue,
    #[serde(default = "default_transform_width")]
    pub width: StyleValue,
    #[serde(default = "default_transform_height")]
    pub height: StyleValue,
    #[serde(default)]
    pub rotation: StyleValue,
    #[serde(default = "default_anchor")]
    pub anchor_x: StyleValue,
    #[serde(default = "default_anchor")]
    pub anchor_y: StyleValue,
    #[serde(default = "default_scale")]
    pub scale_x: StyleValue,
    #[serde(default = "default_scale")]
    pub scale_y: StyleValue,
    #[serde(default)]
    pub skew_x: StyleValue,
    #[serde(default)]
    pub skew_y: StyleValue,
}

impl Default for TransformStyle {
    fn default() -> Self {
        Self {
            x: StyleValue::Value(0.0),
            y: StyleValue::Value(0.0),
            width: default_transform_width(),
            height: default_transform_height(),
            rotation: StyleValue::Value(0.0),
            anchor_x: default_anchor(),
            anchor_y: default_anchor(),
            scale_x: default_scale(),
            scale_y: default_scale(),
            skew_x: StyleValue::Value(0.0),
            skew_y: StyleValue::Value(0.0),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

impl Default for TextAlign {
    fn default() -> Self {
        Self::Center
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerticalAlign {
    Top,
    Middle,
    Bottom,
}

impl Default for VerticalAlign {
    fn default() -> Self {
        Self::Middle
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FitMode {
    Cover,
    Contain,
    Fill,
    None,
}

impl Default for FitMode {
    fn default() -> Self {
        Self::Cover
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct StrokeStyle {
    pub color: [u8; 4],
    #[serde(default = "default_stroke_width")]
    pub width: StyleValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dash: Option<StrokeDashStyle>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct StrokeDashStyle {
    pub pattern: Vec<StyleValue>,
    #[serde(default)]
    pub offset: StyleValue,
}

// Note: deny_unknown_fields is intentionally omitted here because
// #[serde(flatten)] on `base` is incompatible with it in serde.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct ClipStyle {
    #[serde(flatten)]
    pub base: BaseStyle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<[u8; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<StrokeStyle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corner_radius: Option<[StyleValue; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<StyleValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_weight: Option<StyleValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[u8; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<TextAlign>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vertical_align: Option<VerticalAlign>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub letter_spacing: Option<StyleValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_height: Option<StyleValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit: Option<FitMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_matrix: Option<[[f32; 5]; 4]>,
}

impl Default for ClipStyle {
    fn default() -> Self {
        Self {
            base: BaseStyle::default(),
            fill: None,
            stroke: None,
            corner_radius: None,
            font_family: None,
            font_size: None,
            font_weight: None,
            color: None,
            align: None,
            vertical_align: None,
            letter_spacing: None,
            line_height: None,
            fit: None,
            color_matrix: None,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_opacity() -> StyleValue {
    StyleValue::Value(1.0)
}

fn default_alignment() -> [StyleValue; 2] {
    [StyleValue::Value(0.0), StyleValue::Value(0.0)]
}

fn default_shadow_blur() -> StyleValue {
    StyleValue::Value(12.0)
}

fn default_shadow_color() -> [u8; 4] {
    [0, 0, 0, 128]
}

fn default_transform_width() -> StyleValue {
    StyleValue::Expr("canvas.width".to_string())
}

fn default_transform_height() -> StyleValue {
    StyleValue::Expr("canvas.height".to_string())
}

fn default_anchor() -> StyleValue {
    StyleValue::Value(0.5)
}

fn default_scale() -> StyleValue {
    StyleValue::Value(1.0)
}

fn default_stroke_width() -> StyleValue {
    StyleValue::Value(1.0)
}

#[cfg(test)]
mod tests {
    use super::StyleValue;

    #[test]
    fn style_value_number_deserializes_as_literal() {
        let value: StyleValue =
            serde_json::from_str("12.5").expect("deserialize literal style value");
        assert_eq!(value, StyleValue::Value(12.5));
    }

    #[test]
    fn style_value_string_deserializes_as_expression() {
        let value: StyleValue = serde_json::from_str("\"timeline.frame / 30.0\"")
            .expect("deserialize expression style value");
        assert_eq!(value, StyleValue::Expr("timeline.frame / 30.0".to_string()));
    }
}
