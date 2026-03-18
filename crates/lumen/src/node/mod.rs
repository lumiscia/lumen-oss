//! Node type system, shared value types, and enum-based node dispatch.

use std::fmt;

use crate::{
    error::{LumenError, PropertyError},
    expr::Expression,
    media::MediaStore,
    raster::RasterFrame,
    render::{context::RenderContext, surface::SurfacePool},
};

pub mod compositing;
pub mod media_output;
pub mod pixel_utils;
pub mod processing;
pub mod source;
pub mod vector;

pub use vector::{
    ShapeGeometry, VectorData, VectorPosition, VectorStroke, VectorStyle, VectorTextData,
};

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputPortDef {
    pub name: &'static str,
    pub kind: PortKind,
    pub optional: bool,
    pub variadic: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputPortDef {
    pub name: &'static str,
    pub kind: PortKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PropertyKind {
    Float = 0,
    Int = 1,
    Bool = 2,
    String = 3,
    Color = 4,
    Vec2 = 5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropertyDef {
    pub name: &'static str,
    pub expected: PropertyKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PortKind {
    RasterFrame = 0,
    Surface = 1,
    Vector = 2,
}

#[derive(Debug, Clone)]
pub enum NodeProperty {
    Float(f64),
    Int(i64),
    Bool(bool),
    String(String),
    Color([u8; 4]),
    Vec2((f64, f64)),
    FloatVec(Vec<f64>),
    IntVec(Vec<i64>),
    StringVec(Vec<String>),
    Expr(Expression),
}

impl NodeProperty {
    fn invalid_type(
        node_id: NodeId,
        property_path: &str,
        expected: &'static str,
        actual: &'static str,
    ) -> LumenError {
        LumenError::Property(PropertyError::InvalidType {
            node_id,
            property_path: property_path.to_string(),
            expected,
            actual,
        })
    }

    pub fn resolve_float(
        &self,
        node_id: NodeId,
        property_path: &str,
        _ctx: &crate::expr::ExpressionContext,
    ) -> crate::Result<f64> {
        match self {
            Self::Float(value) => Ok(*value),
            Self::Int(value) => Ok(*value as f64),
            Self::String(value) => value
                .parse::<f64>()
                .map_err(|_| Self::invalid_type(node_id, property_path, "Float", "String")),
            _ => Err(Self::invalid_type(
                node_id,
                property_path,
                "Float",
                "unsupported",
            )),
        }
    }

    pub fn resolve_int(
        &self,
        node_id: NodeId,
        property_path: &str,
        _ctx: &crate::expr::ExpressionContext,
    ) -> crate::Result<i64> {
        match self {
            Self::Int(value) => Ok(*value),
            Self::Float(value) => Ok(*value as i64),
            Self::Bool(value) => Ok(i64::from(*value)),
            Self::String(value) => value
                .parse::<i64>()
                .map_err(|_| Self::invalid_type(node_id, property_path, "Int", "String")),
            _ => Err(Self::invalid_type(
                node_id,
                property_path,
                "Int",
                "unsupported",
            )),
        }
    }

    pub fn resolve_bool(
        &self,
        node_id: NodeId,
        property_path: &str,
        _ctx: &crate::expr::ExpressionContext,
    ) -> crate::Result<bool> {
        match self {
            Self::Bool(value) => Ok(*value),
            Self::Int(value) => Ok(*value != 0),
            Self::Float(value) => Ok(*value != 0.0),
            Self::String(value) => match value.to_ascii_lowercase().as_str() {
                "true" | "1" => Ok(true),
                "false" | "0" => Ok(false),
                _ => Err(Self::invalid_type(node_id, property_path, "Bool", "String")),
            },
            _ => Err(Self::invalid_type(
                node_id,
                property_path,
                "Bool",
                "unsupported",
            )),
        }
    }

    pub fn resolve_string(
        &self,
        node_id: NodeId,
        property_path: &str,
        _ctx: &crate::expr::ExpressionContext,
    ) -> crate::Result<String> {
        match self {
            Self::String(value) => Ok(value.clone()),
            Self::Int(value) => Ok(value.to_string()),
            Self::Float(value) => Ok(value.to_string()),
            Self::Bool(value) => Ok(value.to_string()),
            _ => Err(Self::invalid_type(
                node_id,
                property_path,
                "String",
                "unsupported",
            )),
        }
    }

    pub fn resolve_color(
        &self,
        node_id: NodeId,
        property_path: &str,
        _ctx: &crate::expr::ExpressionContext,
    ) -> crate::Result<[u8; 4]> {
        match self {
            Self::Color(value) => Ok(*value),
            _ => Err(Self::invalid_type(
                node_id,
                property_path,
                "Color",
                "unsupported",
            )),
        }
    }

    pub fn resolve_vec2(
        &self,
        node_id: NodeId,
        property_path: &str,
        _ctx: &crate::expr::ExpressionContext,
    ) -> crate::Result<(f64, f64)> {
        match self {
            Self::Vec2(value) => Ok(*value),
            _ => Err(Self::invalid_type(
                node_id,
                property_path,
                "Vec2",
                "unsupported",
            )),
        }
    }
}

#[derive(Debug)]
pub enum NodeResult {
    Raster(RasterFrame),
    Vector(VectorData),
    None,
}

impl NodeResult {
    pub fn as_raster(&self) -> crate::Result<&RasterFrame> {
        match self {
            Self::Raster(frame) => Ok(frame),
            Self::Vector(_) | Self::None => Err(LumenError::Property(PropertyError::InvalidType {
                node_id: NodeId::new(0),
                property_path: "result".to_string(),
                expected: "RasterFrame",
                actual: "non-raster",
            })),
        }
    }

    pub fn as_vector(&self) -> crate::Result<&VectorData> {
        match self {
            Self::Vector(vector) => Ok(vector),
            Self::Raster(_) | Self::None => Err(LumenError::Property(PropertyError::InvalidType {
                node_id: NodeId::new(0),
                property_path: "result".to_string(),
                expected: "Vector",
                actual: "non-vector",
            })),
        }
    }
}

impl From<RasterFrame> for NodeResult {
    fn from(value: RasterFrame) -> Self {
        Self::Raster(value)
    }
}

impl From<VectorData> for NodeResult {
    fn from(value: VectorData) -> Self {
        Self::Vector(value)
    }
}

pub trait PropertyEval {
    fn property_defs(&self) -> &'static [PropertyDef];

    fn get_property(&self, id: &str) -> crate::Result<Option<NodeProperty>>;
}

pub trait NodeDef {
    fn property_defs() -> &'static [PropertyDef];

    fn input_port_defs() -> &'static [InputPortDef];

    fn output_port_defs() -> &'static [OutputPortDef];
}

pub trait Node: PropertyEval + Send + Sync {
    fn id(&self) -> NodeId;
    fn input_port_defs(&self) -> &'static [InputPortDef];
    fn output_port_defs(&self) -> &'static [OutputPortDef];
}

pub trait NodeEval<'a, S: SurfacePool, M: MediaStore>: PropertyEval + Send + Sync {
    fn evaluate(
        &self,
        context: &mut RenderContext<'a, S, M>,
        port: &str,
    ) -> crate::Result<NodeResult>;
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PortRef {
    pub id: NodeId,
    pub port: String,
}

impl PortRef {
    pub fn new(id: NodeId, port: String) -> Self {
        Self { id, port }
    }

    pub fn empty() -> Self {
        Self {
            id: NodeId::new(0),
            port: Default::default(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.id.0 == 0
    }
}

#[derive(Debug)]
pub enum NodeKind {
    // compositing nodes
    Boolean(compositing::boolean::Boolean),
    Merge(compositing::merge::Merge),
    RasterMultimerge(compositing::raster_multimerge::RasterMultiMerge),
    Switch(compositing::switch::Switch),

    // processing nodes
    Blur(processing::blur::Blur),
    Crop(processing::crop::Crop),
    FrameHold(processing::frame_hold::FrameHold),
    Memo(processing::memo::Memo),
    Resize(processing::resize::Resize),
    Shadow(processing::shadow::Shadow),
    Transform(processing::transform::Transform),

    // source nodes
    MediaIn(source::media_in::MediaIn),
    SolidColor(source::solid_color::SolidColor),
    Text(source::text::Text),

    // vector nodes
    Shape(vector::shape::Shape),
    ShapeRenderer(vector::shape_renderer::ShapeRenderer),
    VectorMultimerge(vector::vector_multimerge::VectorMultiMerge),
    VectorText(vector::vector_text::VectorText),

    MediaOutput(media_output::MediaOutput),
}

impl NodeKind {
    pub fn as_node(&self) -> &dyn Node {
        match self {
            NodeKind::Boolean(boolean) => boolean,
            NodeKind::Merge(merge) => merge,
            NodeKind::RasterMultimerge(raster_multi_merge) => raster_multi_merge,
            NodeKind::Switch(switch) => switch,
            NodeKind::Blur(blur) => blur,
            NodeKind::Crop(crop) => crop,
            NodeKind::FrameHold(frame_hold) => frame_hold,
            NodeKind::Memo(memo) => memo,
            NodeKind::Resize(resize) => resize,
            NodeKind::Shadow(shadow) => shadow,
            NodeKind::Transform(transform) => transform,
            NodeKind::MediaIn(media_in) => media_in,
            NodeKind::SolidColor(solid_color) => solid_color,
            NodeKind::Text(text) => text,
            NodeKind::Shape(shape) => shape,
            NodeKind::ShapeRenderer(shape_renderer) => shape_renderer,
            NodeKind::VectorMultimerge(vector_multi_merge) => vector_multi_merge,
            NodeKind::VectorText(vector_text) => vector_text,
            NodeKind::MediaOutput(media_output) => media_output,
        }
    }

    pub fn as_property_eval(&self) -> &dyn PropertyEval {
        match self {
            NodeKind::Boolean(boolean) => boolean,
            NodeKind::Merge(merge) => merge,
            NodeKind::RasterMultimerge(raster_multi_merge) => raster_multi_merge,
            NodeKind::Switch(switch) => switch,
            NodeKind::Blur(blur) => blur,
            NodeKind::Crop(crop) => crop,
            NodeKind::FrameHold(frame_hold) => frame_hold,
            NodeKind::Memo(memo) => memo,
            NodeKind::Resize(resize) => resize,
            NodeKind::Shadow(shadow) => shadow,
            NodeKind::Transform(transform) => transform,
            NodeKind::MediaIn(media_in) => media_in,
            NodeKind::SolidColor(solid_color) => solid_color,
            NodeKind::Text(text) => text,
            NodeKind::Shape(shape) => shape,
            NodeKind::ShapeRenderer(shape_renderer) => shape_renderer,
            NodeKind::VectorMultimerge(vector_multi_merge) => vector_multi_merge,
            NodeKind::VectorText(vector_text) => vector_text,
            NodeKind::MediaOutput(media_output) => media_output,
        }
    }

    pub fn as_node_eval<'a, S: SurfacePool, M: MediaStore>(&self) -> &dyn NodeEval<'a, S, M> {
        match self {
            NodeKind::Boolean(boolean) => boolean,
            NodeKind::Merge(merge) => merge,
            NodeKind::RasterMultimerge(raster_multi_merge) => raster_multi_merge,
            NodeKind::Switch(switch) => switch,
            NodeKind::Blur(blur) => blur,
            NodeKind::Crop(crop) => crop,
            NodeKind::FrameHold(frame_hold) => frame_hold,
            NodeKind::Memo(memo) => memo,
            NodeKind::Resize(resize) => resize,
            NodeKind::Shadow(shadow) => shadow,
            NodeKind::Transform(transform) => transform,
            NodeKind::MediaIn(media_in) => media_in,
            NodeKind::SolidColor(solid_color) => solid_color,
            NodeKind::Text(text) => text,
            NodeKind::Shape(shape) => shape,
            NodeKind::ShapeRenderer(shape_renderer) => shape_renderer,
            NodeKind::VectorMultimerge(vector_multi_merge) => vector_multi_merge,
            NodeKind::VectorText(vector_text) => vector_text,
            NodeKind::MediaOutput(media_output) => media_output,
        }
    }
}

// such small performance loss for more readable code using dyn, don't really care
impl PropertyEval for NodeKind {
    fn property_defs(&self) -> &'static [PropertyDef] {
        self.as_property_eval().property_defs()
    }

    fn get_property(&self, id: &str) -> crate::Result<Option<NodeProperty>> {
        self.as_property_eval().get_property(id)
    }
}

impl Node for NodeKind {
    fn id(&self) -> NodeId {
        self.as_node().id()
    }

    fn input_port_defs(&self) -> &'static [InputPortDef] {
        self.as_node().input_port_defs()
    }

    fn output_port_defs(&self) -> &'static [OutputPortDef] {
        self.as_node().output_port_defs()
    }
}

impl<'a, S: SurfacePool, M: MediaStore> NodeEval<'a, S, M> for NodeKind {
    fn evaluate(
        &self,
        context: &mut RenderContext<'a, S, M>,
        port: &str,
    ) -> crate::Result<NodeResult> {
        self.as_node_eval().evaluate(context, port)
    }
}
