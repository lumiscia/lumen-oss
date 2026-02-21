use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum StyleValue {
    Value(f32),
    Expr(String),
}

impl Default for StyleValue {
    fn default() -> Self {
        Self::Value(0.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct BaseStyle {
    #[serde(default = "default_visible")]
    pub visible: bool,
    #[serde(default = "default_opacity")]
    pub opacity: StyleValue,
    #[serde(default)]
    pub transform: TransformStyle,
}

impl Default for BaseStyle {
    fn default() -> Self {
        Self {
            visible: default_visible(),
            opacity: default_opacity(),
            transform: TransformStyle::default(),
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
    #[serde(default = "default_width")]
    pub width: StyleValue,
    #[serde(default = "default_height")]
    pub height: StyleValue,
    #[serde(default)]
    pub rotation: StyleValue,
}

impl Default for TransformStyle {
    fn default() -> Self {
        Self {
            x: StyleValue::Value(0.0),
            y: StyleValue::Value(0.0),
            width: default_width(),
            height: default_height(),
            rotation: StyleValue::Value(0.0),
        }
    }
}

fn default_visible() -> bool {
    true
}

fn default_opacity() -> StyleValue {
    StyleValue::Value(1.0)
}

fn default_width() -> StyleValue {
    StyleValue::Expr("canvas.width".to_string())
}

fn default_height() -> StyleValue {
    StyleValue::Expr("canvas.height".to_string())
}
