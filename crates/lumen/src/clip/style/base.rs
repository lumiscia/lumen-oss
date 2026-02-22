use skia_safe::BlendMode;

use crate::clip::style::StyleProperty;

#[derive(Debug, Clone)]
pub struct BaseStyle {
    pub visible: StyleProperty<bool>,
    pub opacity: StyleProperty<f32>,
    pub blend_mode: BlendMode,
    pub blur: StyleProperty<f32>,
    pub shadow: Option<ShadowStyle>,
    pub transform: TransformStyle,
    pub alignment: [StyleProperty<f32>; 2],
}

#[derive(Debug, Clone, PartialEq)]
pub struct TransformStyle {
    pub translate: StyleProperty<f32>,
    pub scale: StyleProperty<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShadowStyle {
    pub offset_x: StyleProperty<f32>,
    pub offset_y: StyleProperty<f32>,
    pub blur: StyleProperty<f32>,
    pub color: [StyleProperty<u8>; 4],
}
