//! Node type system, shared value types, and enum-based node dispatch.

use std::{collections::HashMap, fmt};

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
    Rectangle { width: u32, height: u32 },
    Ellipse { width: u32, height: u32 },
    Polygon { points: Vec<(f32, f32)> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum VectorData {
    Shape(ShapeGeometry),
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
