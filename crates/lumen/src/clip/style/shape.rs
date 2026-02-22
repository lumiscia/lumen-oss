use crate::clip::style::{BaseStyle, StyleProperty};

#[derive(Debug, Clone)]
pub struct EllipseStyle {
    pub base: BaseStyle,
    pub width: StyleProperty<f32>,
    pub height: StyleProperty<f32>,
}

#[derive(Debug, Clone)]
pub struct RectStyle {
    pub base: BaseStyle,
    pub width: StyleProperty<f32>,
    pub height: StyleProperty<f32>,
    pub corner_radius: [StyleProperty<f32>; 4],
}

#[derive(Debug, Clone)]
pub struct PolygonStyle {
    pub base: BaseStyle,
    pub width: StyleProperty<f32>,
    pub height: StyleProperty<f32>,
    pub sides: StyleProperty<u32>,
}
