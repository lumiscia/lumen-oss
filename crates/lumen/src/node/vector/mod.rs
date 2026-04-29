pub mod path;
pub mod shape;
pub mod shape_renderer;
pub mod vector_merge;
pub mod vector_multimerge;
pub mod vector_stroke_style;
pub mod vector_text;
pub mod vector_transform;

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
    Path {
        commands: String,
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VectorTransformData {
    pub translate: VectorPosition,
    pub scale_x: f32,
    pub scale_y: f32,
    pub rotate: f32,
    pub pivot: VectorPosition,
}

impl Default for VectorTransformData {
    fn default() -> Self {
        Self {
            translate: VectorPosition::default(),
            scale_x: 1.0,
            scale_y: 1.0,
            rotate: 0.0,
            pivot: VectorPosition::default(),
        }
    }
}

impl VectorTransformData {
    pub fn is_identity(self) -> bool {
        self.translate.x.abs() <= f32::EPSILON
            && self.translate.y.abs() <= f32::EPSILON
            && (self.scale_x - 1.0).abs() <= f32::EPSILON
            && (self.scale_y - 1.0).abs() <= f32::EPSILON
            && self.rotate.abs() <= f32::EPSILON
    }
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
    Transformed {
        child: Box<VectorData>,
        transform: VectorTransformData,
    },
}
