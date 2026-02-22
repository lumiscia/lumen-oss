use skia_safe::paint::{Cap as StrokeCap, Join as StrokeJoin};

use crate::clip::style::{BaseStyle, StyleProperty};

#[derive(Debug, Clone)]
pub enum Fill {
    Solid { color: [StyleProperty<u8>; 4] },
}

#[derive(Debug, Clone)]
pub struct Stroke {
    pub color: [StyleProperty<u8>; 4],
    pub width: StyleProperty<f32>,
    pub dash_pattern: Option<Vec<f32>>,
    pub line_cap: StrokeCap,
    pub line_join: StrokeJoin,
}

#[derive(Debug, Clone)]
pub struct EllipseStyle {
    pub base: BaseStyle,
    pub width: StyleProperty<f32>,
    pub height: StyleProperty<f32>,
    pub fill: Option<Fill>,
    pub stroke: Option<Stroke>,
}

#[derive(Debug, Clone)]
pub struct RectStyle {
    pub base: BaseStyle,
    pub width: StyleProperty<f32>,
    pub height: StyleProperty<f32>,
    pub corner_radius: [StyleProperty<f32>; 4],
    pub fill: Option<Fill>,
    pub stroke: Option<Stroke>,
}

#[derive(Debug, Clone)]
pub struct PolygonStyle {
    pub base: BaseStyle,
    pub width: StyleProperty<f32>,
    pub height: StyleProperty<f32>,
    pub sides: StyleProperty<u32>,
    pub fill: Option<Fill>,
    pub stroke: Option<Stroke>,
}
