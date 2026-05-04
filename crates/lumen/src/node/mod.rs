//! Renderer-agnostic node schema and shared parameter types.
//!
//! Node structs stay intentionally small here: they describe graph shape and
//! animatable parameters. GPU lowering lives in `crate::gpu`.

use std::fmt;

use crate::{
    error::{LumenError, PropertyError},
    expr::Expression,
};

pub mod compositing;
pub mod media_output;
pub mod processing;
pub mod source;
pub mod vector;

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
pub enum NodeCategory {
    Compositing = 0,
    Processing = 1,
    Source = 2,
    Output = 3,
    Vector = 4,
}

#[derive(Debug, Clone)]
pub struct NodeSchemaDef {
    pub kind: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub category: NodeCategory,
    pub inputs: &'static [InputPortDef],
    pub properties: &'static [PropertyDef],
    pub default_properties: Vec<(&'static str, NodeProperty)>,
}

pub trait NodeSchema: Default {
    fn schema() -> NodeSchemaDef;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PortKind {
    Raster = 0,
    Vector = 1,
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
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<f64> {
        match self {
            Self::Float(value) => Ok(*value),
            Self::Int(value) => Ok(*value as f64),
            Self::String(value) => value
                .parse::<f64>()
                .map_err(|_| Self::invalid_type(node_id, property_path, "Float", "String")),
            Self::Expr(expr) => expr
                .evaluate(ctx)?
                .as_f64()
                .ok_or_else(|| Self::invalid_type(node_id, property_path, "Float", "expression")),
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
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<i64> {
        match self {
            Self::Int(value) => Ok(*value),
            Self::Float(value) => Ok(*value as i64),
            Self::Bool(value) => Ok(i64::from(*value)),
            Self::String(value) => value
                .parse::<i64>()
                .map_err(|_| Self::invalid_type(node_id, property_path, "Int", "String")),
            Self::Expr(expr) => {
                let value = expr.evaluate(ctx)?.as_f64().ok_or_else(|| {
                    Self::invalid_type(node_id, property_path, "Int", "expression")
                })?;
                Ok(value as i64)
            }
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
        ctx: &crate::expr::ExpressionContext<'_>,
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
            Self::Expr(expr) => Ok(expr.evaluate(ctx)?.as_bool()),
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
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<String> {
        match self {
            Self::String(value) => Ok(value.clone()),
            Self::Int(value) => Ok(value.to_string()),
            Self::Float(value) => Ok(value.to_string()),
            Self::Bool(value) => Ok(value.to_string()),
            Self::Expr(expr) => Ok(expr.evaluate(ctx)?.as_string()),
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
        _ctx: &crate::expr::ExpressionContext<'_>,
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
        _ctx: &crate::expr::ExpressionContext<'_>,
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

pub trait Node: Send + Sync {
    fn id(&self) -> NodeId;
    fn input_port_defs(&self) -> &'static [InputPortDef];
    fn output_port_defs(&self) -> &'static [OutputPortDef] {
        SINGLE_RASTER_OUTPUT
    }
}

pub trait PropertyEval {
    fn get_property(&self, id: &str) -> crate::Result<Option<NodeProperty>>;
}

pub const SINGLE_RASTER_OUTPUT: &[OutputPortDef] = &[OutputPortDef {
    name: "output",
    kind: PortKind::Raster,
}];

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
            port: String::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.id.0 == 0
    }
}

#[derive(Debug)]
pub enum NodeKind {
    MediaIn(source::media_in::MediaIn),
    SolidColor(source::solid_color::SolidColor),
    Text(source::text::Text),
    Path(vector::path::Path),
    Shape(vector::shape::Shape),
    Boolean(compositing::boolean::Boolean),
    Merge(compositing::merge::Merge),
    RasterMultiMerge(compositing::raster_multimerge::RasterMultiMerge),
    AlphaPremultiply(processing::alpha_premultiply::AlphaPremultiply),
    Blur(processing::blur::Blur),
    ChannelShuffle(processing::channel_shuffle::ChannelShuffle),
    ColorGrade(processing::color_grade::ColorGrade),
    Curves(processing::curves::Curves),
    Exposure(processing::exposure::Exposure),
    HueSaturation(processing::hue_saturation::HueSaturation),
    Levels(processing::levels::Levels),
    Memo(processing::memo::Memo),
    TimeRemap(processing::time_remap::TimeRemap),
    Transform(processing::transform::Transform),
    Crop(processing::crop::Crop),
    Resize(processing::resize::Resize),
    Shadow(processing::shadow::Shadow),
    WgslShader(processing::wgsl_shader::WgslShader),
    Switch(compositing::switch::Switch),
    MediaOutput(media_output::MediaOutput),
}

impl NodeKind {
    pub fn id(&self) -> NodeId {
        match self {
            Self::MediaIn(node) => node.id,
            Self::SolidColor(node) => node.id,
            Self::Text(node) => node.id,
            Self::Path(node) => node.id,
            Self::Shape(node) => node.id,
            Self::Boolean(node) => node.id,
            Self::Merge(node) => node.id,
            Self::RasterMultiMerge(node) => node.id,
            Self::AlphaPremultiply(node) => node.id,
            Self::Blur(node) => node.id,
            Self::ChannelShuffle(node) => node.id,
            Self::ColorGrade(node) => node.id,
            Self::Curves(node) => node.id,
            Self::Exposure(node) => node.id,
            Self::HueSaturation(node) => node.id,
            Self::Levels(node) => node.id,
            Self::Memo(node) => node.id,
            Self::TimeRemap(node) => node.id,
            Self::Transform(node) => node.id,
            Self::Crop(node) => node.id,
            Self::Resize(node) => node.id,
            Self::Shadow(node) => node.id,
            Self::WgslShader(node) => node.id,
            Self::Switch(node) => node.id,
            Self::MediaOutput(node) => node.id,
        }
    }

    pub fn as_property_eval(&self) -> &dyn PropertyEval {
        self
    }

    pub fn schemas() -> Vec<NodeSchemaDef> {
        vec![
            source::media_in::MediaIn::schema(),
            source::solid_color::SolidColor::schema(),
            source::text::Text::schema(),
            vector::path::Path::schema(),
            vector::shape::Shape::schema(),
            compositing::boolean::Boolean::schema(),
            compositing::merge::Merge::schema(),
            compositing::raster_multimerge::RasterMultiMerge::schema(),
            compositing::switch::Switch::schema(),
            processing::memo::Memo::schema(),
            processing::alpha_premultiply::AlphaPremultiply::schema(),
            processing::blur::Blur::schema(),
            processing::channel_shuffle::ChannelShuffle::schema(),
            processing::color_grade::ColorGrade::schema(),
            processing::curves::Curves::schema(),
            processing::exposure::Exposure::schema(),
            processing::hue_saturation::HueSaturation::schema(),
            processing::levels::Levels::schema(),
            processing::time_remap::TimeRemap::schema(),
            processing::transform::Transform::schema(),
            processing::crop::Crop::schema(),
            processing::resize::Resize::schema(),
            processing::shadow::Shadow::schema(),
            processing::wgsl_shader::WgslShader::schema(),
            media_output::MediaOutput::schema(),
        ]
    }
}

impl Node for NodeKind {
    fn id(&self) -> NodeId {
        self.id()
    }

    fn input_port_defs(&self) -> &'static [InputPortDef] {
        match self {
            Self::MediaIn(node) => node.input_port_defs(),
            Self::SolidColor(node) => node.input_port_defs(),
            Self::Text(node) => node.input_port_defs(),
            Self::Path(node) => node.input_port_defs(),
            Self::Shape(node) => node.input_port_defs(),
            Self::Boolean(node) => node.input_port_defs(),
            Self::Merge(node) => node.input_port_defs(),
            Self::RasterMultiMerge(node) => node.input_port_defs(),
            Self::AlphaPremultiply(node) => node.input_port_defs(),
            Self::Blur(node) => node.input_port_defs(),
            Self::ChannelShuffle(node) => node.input_port_defs(),
            Self::ColorGrade(node) => node.input_port_defs(),
            Self::Curves(node) => node.input_port_defs(),
            Self::Exposure(node) => node.input_port_defs(),
            Self::HueSaturation(node) => node.input_port_defs(),
            Self::Levels(node) => node.input_port_defs(),
            Self::Memo(node) => node.input_port_defs(),
            Self::TimeRemap(node) => node.input_port_defs(),
            Self::Transform(node) => node.input_port_defs(),
            Self::Crop(node) => node.input_port_defs(),
            Self::Resize(node) => node.input_port_defs(),
            Self::Shadow(node) => node.input_port_defs(),
            Self::WgslShader(node) => node.input_port_defs(),
            Self::Switch(node) => node.input_port_defs(),
            Self::MediaOutput(node) => node.input_port_defs(),
        }
    }

    fn output_port_defs(&self) -> &'static [OutputPortDef] {
        SINGLE_RASTER_OUTPUT
    }
}

impl PropertyEval for NodeKind {
    fn get_property(&self, id: &str) -> crate::Result<Option<NodeProperty>> {
        match self {
            Self::MediaIn(node) => node.get_property(id),
            Self::SolidColor(node) => node.get_property(id),
            Self::Text(node) => node.get_property(id),
            Self::Path(node) => node.get_property(id),
            Self::Shape(node) => node.get_property(id),
            Self::Boolean(node) => node.get_property(id),
            Self::Merge(node) => node.get_property(id),
            Self::RasterMultiMerge(node) => node.get_property(id),
            Self::AlphaPremultiply(node) => node.get_property(id),
            Self::Blur(node) => node.get_property(id),
            Self::ChannelShuffle(node) => node.get_property(id),
            Self::ColorGrade(node) => node.get_property(id),
            Self::Curves(node) => node.get_property(id),
            Self::Exposure(node) => node.get_property(id),
            Self::HueSaturation(node) => node.get_property(id),
            Self::Levels(node) => node.get_property(id),
            Self::Memo(node) => node.get_property(id),
            Self::TimeRemap(node) => node.get_property(id),
            Self::Transform(node) => node.get_property(id),
            Self::Crop(node) => node.get_property(id),
            Self::Resize(node) => node.get_property(id),
            Self::Shadow(node) => node.get_property(id),
            Self::WgslShader(node) => node.get_property(id),
            Self::Switch(node) => node.get_property(id),
            Self::MediaOutput(node) => node.get_property(id),
        }
    }
}
