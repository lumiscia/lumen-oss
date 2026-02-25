//! Node type system, shared value types, and enum-based node dispatch.

use std::{collections::HashMap, fmt, hash::Hasher};

use crate::{
    error::{LumenError, PropertyError},
    raster::RasterFrame,
    render::RenderContext,
};

pub mod blur;
pub mod boolean;
pub mod crop;
pub mod frame_hold;
pub mod media_in;
pub mod media_output;
pub mod memo;
pub mod merge;
pub mod pixel_utils;
pub mod resize;
pub mod shadow;
pub mod shape;
pub mod shape_renderer;
pub mod solid_color;
pub mod switch;
pub mod text;
pub mod transform;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u64);

impl NodeId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TrackId(pub u64);

impl TrackId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
}

impl fmt::Display for TrackId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PortKind {
    RasterFrame = 0,
    Surface = 1,
    Vector = 2,
}

#[derive(Debug, Clone)]
pub enum PortValue {
    RasterFrame(RasterFrame),
    Vector(VectorData),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShapeGeometry {
    Rectangle {
        width: u32,
        height: u32,
        border_radius: f32,
    },
    Ellipse { width: u32, height: u32 },
    Polygon { points: Vec<(f32, f32)> },
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

#[derive(Debug, Clone, PartialEq)]
pub enum VectorData {
    Shape {
        geometry: ShapeGeometry,
        style: VectorStyle,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputPortDef {
    pub name: &'static str,
    pub kind: PortKind,
    pub optional: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputPortDef {
    pub name: &'static str,
    pub kind: PortKind,
}

#[derive(Debug, Clone, Default)]
pub struct NodeInputs {
    ports: HashMap<String, PortValue>,
}

impl NodeInputs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, name: impl Into<String>, value: PortValue) {
        self.ports.insert(name.into(), value);
    }

    pub fn get_port(&self, name: &str) -> Option<&PortValue> {
        self.ports.get(name)
    }

    pub fn get_raster(&self, name: &str) -> Result<&RasterFrame, LumenError> {
        match self.ports.get(name) {
            Some(PortValue::RasterFrame(frame)) => Ok(frame),
            Some(_) => Err(PropertyError::InvalidType {
                node_id: NodeId(0),
                property_path: name.to_string(),
                expected: "RasterFrame",
                actual: "non-raster",
            }
            .into()),
            None => Err(PropertyError::MissingProperty {
                node_id: NodeId(0),
                property_path: name.to_string(),
            }
            .into()),
        }
    }

    pub fn get_raster_optional(&self, name: &str) -> Result<Option<&RasterFrame>, LumenError> {
        match self.ports.get(name) {
            Some(PortValue::RasterFrame(frame)) => Ok(Some(frame)),
            Some(_) => Err(PropertyError::InvalidType {
                node_id: NodeId(0),
                property_path: name.to_string(),
                expected: "RasterFrame",
                actual: "non-raster",
            }
            .into()),
            None => Ok(None),
        }
    }

    pub fn get_vector(&self, name: &str) -> Result<&VectorData, LumenError> {
        match self.ports.get(name) {
            Some(PortValue::Vector(vector)) => Ok(vector),
            Some(_) => Err(PropertyError::InvalidType {
                node_id: NodeId(0),
                property_path: name.to_string(),
                expected: "Vector",
                actual: "non-vector",
            }
            .into()),
            None => Err(PropertyError::MissingProperty {
                node_id: NodeId(0),
                property_path: name.to_string(),
            }
            .into()),
        }
    }

    pub fn get_vector_optional(&self, name: &str) -> Result<Option<&VectorData>, LumenError> {
        match self.ports.get(name) {
            Some(PortValue::Vector(vector)) => Ok(Some(vector)),
            Some(_) => Err(PropertyError::InvalidType {
                node_id: NodeId(0),
                property_path: name.to_string(),
                expected: "Vector",
                actual: "non-vector",
            }
            .into()),
            None => Ok(None),
        }
    }
}

pub trait NodeEval: Send + Sync {
    fn input_port_defs(&self) -> &'static [InputPortDef];
    fn output_port_defs(&self) -> &'static [OutputPortDef];
    fn evaluate(
        &self,
        inputs: &NodeInputs,
        ctx: &mut RenderContext,
    ) -> Result<PortValue, LumenError>;
}

#[derive(Debug, Clone, PartialEq)]
pub enum PropertyValue {
    Float(f64),
    Int(i64),
    Bool(bool),
    Color([u8; 4]),
    String(String),
    Vector2(f64, f64),
    Map(HashMap<String, PropertyValue>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
#[non_exhaustive]
pub enum BlendMode {
    Normal = 0,
    Multiply = 1,
    Screen = 2,
    Overlay = 3,
    Darken = 4,
    Lighten = 5,
}

impl From<BlendMode> for skia_safe::BlendMode {
    fn from(value: BlendMode) -> Self {
        match value {
            BlendMode::Normal => Self::SrcOver,
            BlendMode::Multiply => Self::Multiply,
            BlendMode::Screen => Self::Screen,
            BlendMode::Overlay => Self::Overlay,
            BlendMode::Darken => Self::Darken,
            BlendMode::Lighten => Self::Lighten,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
}

impl Node {
    pub fn new(id: NodeId, kind: NodeKind) -> Self {
        Self { id, kind }
    }
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum NodeKind {
    Shape(shape::Shape),
    ShapeRenderer(shape_renderer::ShapeRenderer),
    MediaIn(media_in::MediaIn),
    SolidColor(solid_color::SolidColor),
    Text(text::Text),
    Transform(transform::Transform),
    Crop(crop::Crop),
    Resize(resize::Resize),
    Blur(blur::Blur),
    Shadow(shadow::Shadow),
    Boolean(boolean::Boolean),
    Merge(merge::Merge),
    Switch(switch::Switch),
    FrameHold(frame_hold::FrameHold),
    MediaOutput(media_output::MediaOutput),
    Memo(memo::Memo),
}

impl NodeKind {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Shape(_) => "Shape",
            Self::ShapeRenderer(_) => "ShapeRenderer",
            Self::MediaIn(_) => "MediaIn",
            Self::SolidColor(_) => "SolidColor",
            Self::Text(_) => "Text",
            Self::Transform(_) => "Transform",
            Self::Crop(_) => "Crop",
            Self::Resize(_) => "Resize",
            Self::Blur(_) => "Blur",
            Self::Shadow(_) => "Shadow",
            Self::Boolean(_) => "Boolean",
            Self::Merge(_) => "Merge",
            Self::Switch(_) => "Switch",
            Self::FrameHold(_) => "FrameHold",
            Self::MediaOutput(_) => "MediaOutput",
            Self::Memo(_) => "Memo",
        }
    }

    /// Hash the structural content of this node kind into the given hasher,
    /// without allocating (replaces the prior `format!("{:?}")` approach).
    pub fn hash_content(&self, hasher: &mut impl Hasher) {
        use std::hash::Hash;
        // Discriminant tag
        std::mem::discriminant(self).hash(hasher);
        match self {
            Self::Shape(s) => {
                match &s.geometry {
                    ShapeGeometry::Rectangle {
                        width,
                        height,
                        border_radius,
                    } => {
                        0u8.hash(hasher);
                        width.hash(hasher);
                        height.hash(hasher);
                        border_radius.to_bits().hash(hasher);
                    }
                    ShapeGeometry::Ellipse { width, height } => {
                        1u8.hash(hasher);
                        width.hash(hasher);
                        height.hash(hasher);
                    }
                    ShapeGeometry::Polygon { points } => {
                        2u8.hash(hasher);
                        for (x, y) in points {
                            x.to_bits().hash(hasher);
                            y.to_bits().hash(hasher);
                        }
                    }
                }

                s.style.color.hash(hasher);
                match s.style.stroke {
                    Some(stroke) => {
                        true.hash(hasher);
                        stroke.color.hash(hasher);
                        stroke.width.to_bits().hash(hasher);
                    }
                    None => false.hash(hasher),
                }
            }
            Self::ShapeRenderer(r) => {
                r.fill_color.hash(hasher);
                r.stroke_color.hash(hasher);
                r.stroke_width.to_bits().hash(hasher);
                r.fill_enabled.hash(hasher);
                r.stroke_enabled.hash(hasher);
            }
            Self::MediaIn(m) => match &m.kind {
                media_in::MediaInKind::Image { source } => {
                    0u8.hash(hasher);
                    source.hash(hasher);
                }
                media_in::MediaInKind::Video {
                    source,
                    range,
                    speed,
                    loop_mode,
                } => {
                    1u8.hash(hasher);
                    source.hash(hasher);
                    range.hash(hasher);
                    speed.to_bits().hash(hasher);
                    std::mem::discriminant(loop_mode).hash(hasher);
                }
            },
            Self::SolidColor(c) => {
                c.color.hash(hasher);
                c.width.hash(hasher);
                c.height.hash(hasher);
            }
            Self::Text(t) => {
                t.content.hash(hasher);
                t.font_family.hash(hasher);
                t.font_size.to_bits().hash(hasher);
                t.font_weight.hash(hasher);
                std::mem::discriminant(&t.font_style).hash(hasher);
                t.max_width.map(|v| v.to_bits()).hash(hasher);
                t.color.hash(hasher);
                std::mem::discriminant(&t.alignment.horizontal).hash(hasher);
                std::mem::discriminant(&t.alignment.vertical).hash(hasher);
            }
            Self::Transform(t) => {
                t.scale_x.to_bits().hash(hasher);
                t.scale_y.to_bits().hash(hasher);
                t.translate_x.to_bits().hash(hasher);
                t.translate_y.to_bits().hash(hasher);
                t.rotate.to_bits().hash(hasher);
                t.pivot_x.to_bits().hash(hasher);
                t.pivot_y.to_bits().hash(hasher);
                std::mem::discriminant(&t.sampling).hash(hasher);
            }
            Self::Crop(c) => {
                c.x.hash(hasher);
                c.y.hash(hasher);
                c.width.hash(hasher);
                c.height.hash(hasher);
            }
            Self::Resize(r) => {
                r.width.hash(hasher);
                r.height.hash(hasher);
                std::mem::discriminant(&r.mode).hash(hasher);
                std::mem::discriminant(&r.sampling).hash(hasher);
            }
            Self::Blur(b) => {
                b.radius.to_bits().hash(hasher);
            }
            Self::Shadow(s) => {
                s.offset_x.hash(hasher);
                s.offset_y.hash(hasher);
                s.color.hash(hasher);
                s.blur_radius.to_bits().hash(hasher);
            }
            Self::Boolean(b) => {
                std::mem::discriminant(&b.mask_kind).hash(hasher);
                b.invert.hash(hasher);
            }
            Self::Merge(m) => {
                std::mem::discriminant(&m.blend_mode).hash(hasher);
                m.opacity.to_bits().hash(hasher);
            }
            Self::Switch(s) => {
                let mut entries: Vec<_> = s.map.iter().collect();
                entries.sort_by_key(|(k, _)| *k);
                for (k, range) in entries {
                    k.hash(hasher);
                    range.start.hash(hasher);
                    range.end.hash(hasher);
                }
            }
            Self::FrameHold(f) => {
                f.hold_frame.hash(hasher);
            }
            Self::MediaOutput(_) => {}
            Self::Memo(m) => {
                m.cache_id.hash(hasher);
                m.allow_expressions.hash(hasher);
            }
        }
    }

    pub fn input_port_defs(&self) -> &'static [InputPortDef] {
        match self {
            Self::Shape(node) => node.input_port_defs(),
            Self::ShapeRenderer(node) => node.input_port_defs(),
            Self::MediaIn(node) => node.input_port_defs(),
            Self::SolidColor(node) => node.input_port_defs(),
            Self::Text(node) => node.input_port_defs(),
            Self::Transform(node) => node.input_port_defs(),
            Self::Crop(node) => node.input_port_defs(),
            Self::Resize(node) => node.input_port_defs(),
            Self::Blur(node) => node.input_port_defs(),
            Self::Shadow(node) => node.input_port_defs(),
            Self::Boolean(node) => node.input_port_defs(),
            Self::Merge(node) => node.input_port_defs(),
            Self::Switch(node) => node.input_port_defs(),
            Self::FrameHold(node) => node.input_port_defs(),
            Self::MediaOutput(node) => node.input_port_defs(),
            Self::Memo(node) => node.input_port_defs(),
        }
    }

    pub fn output_port_defs(&self) -> &'static [OutputPortDef] {
        match self {
            Self::Shape(node) => node.output_port_defs(),
            Self::ShapeRenderer(node) => node.output_port_defs(),
            Self::MediaIn(node) => node.output_port_defs(),
            Self::SolidColor(node) => node.output_port_defs(),
            Self::Text(node) => node.output_port_defs(),
            Self::Transform(node) => node.output_port_defs(),
            Self::Crop(node) => node.output_port_defs(),
            Self::Resize(node) => node.output_port_defs(),
            Self::Blur(node) => node.output_port_defs(),
            Self::Shadow(node) => node.output_port_defs(),
            Self::Boolean(node) => node.output_port_defs(),
            Self::Merge(node) => node.output_port_defs(),
            Self::Switch(node) => node.output_port_defs(),
            Self::FrameHold(node) => node.output_port_defs(),
            Self::MediaOutput(node) => node.output_port_defs(),
            Self::Memo(node) => node.output_port_defs(),
        }
    }

    pub fn evaluate(
        &self,
        inputs: &NodeInputs,
        ctx: &mut RenderContext,
    ) -> Result<PortValue, LumenError> {
        match self {
            Self::Shape(node) => node.evaluate(inputs, ctx),
            Self::ShapeRenderer(node) => node.evaluate(inputs, ctx),
            Self::MediaIn(node) => node.evaluate(inputs, ctx),
            Self::SolidColor(node) => node.evaluate(inputs, ctx),
            Self::Text(node) => node.evaluate(inputs, ctx),
            Self::Transform(node) => node.evaluate(inputs, ctx),
            Self::Crop(node) => node.evaluate(inputs, ctx),
            Self::Resize(node) => node.evaluate(inputs, ctx),
            Self::Blur(node) => node.evaluate(inputs, ctx),
            Self::Shadow(node) => node.evaluate(inputs, ctx),
            Self::Boolean(node) => node.evaluate(inputs, ctx),
            Self::Merge(node) => node.evaluate(inputs, ctx),
            Self::Switch(node) => node.evaluate(inputs, ctx),
            Self::FrameHold(node) => node.evaluate(inputs, ctx),
            Self::MediaOutput(node) => node.evaluate(inputs, ctx),
            Self::Memo(node) => node.evaluate(inputs, ctx),
        }
    }
}
