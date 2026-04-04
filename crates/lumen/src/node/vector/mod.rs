pub mod shape;
pub mod shape_renderer;
pub mod vector_merge;
pub mod vector_multimerge;
pub mod vector_text;

use crate::node::source::text;

#[derive(Debug, Clone, PartialEq)]
pub enum ShapeGeometry {
    Rectangle {
        width: u32,
        height: u32,
        border_radius: f32,
    },
    Ellipse {
        width: u32,
        height: u32,
    },
    Polygon {
        points: Vec<(f32, f32)>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VectorStroke {
    pub color: [u8; 4],
    pub width: f32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct VectorStyle {
    pub color: Option<[u8; 4]>,
    pub stroke: Option<VectorStroke>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct VectorPosition {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VectorTextData {
    pub content: String,
    pub font_family: String,
    pub font_size: f32,
    pub font_weight: u16,
    pub font_style: text::TextFontStyle,
    pub max_width: Option<f32>,
    pub alignment: text::TextAlignment,
    pub position: VectorPosition,
    pub style: VectorStyle,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VectorData {
    Shape {
        geometry: ShapeGeometry,
        style: VectorStyle,
        position: VectorPosition,
    },
    Text(VectorTextData),
    Group {
        children: Vec<VectorData>,
        position: VectorPosition,
    },
}
