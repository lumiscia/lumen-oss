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
    Enum = 6,
}

#[cfg(any(feature = "json", feature = "metadata"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnumOptionDef {
    pub name: &'static str,
    pub label: &'static str,
    pub value: i64,
}

#[cfg(any(feature = "json", feature = "metadata"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnumDef {
    pub name: &'static str,
    pub options: &'static [EnumOptionDef],
}

#[cfg(any(feature = "json", feature = "metadata"))]
pub trait NodeEnum {
    fn enum_def() -> &'static EnumDef;
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PropertyDef {
    pub id: &'static str,
    pub expected: PropertyKind,
    #[cfg(any(feature = "json", feature = "metadata"))]
    pub enum_def: Option<&'static EnumDef>,
    #[cfg(feature = "metadata")]
    pub name: &'static str,
    #[cfg(feature = "metadata")]
    pub description: &'static str,
    #[cfg(feature = "metadata")]
    pub constraints: PropertyConstraints,
}

#[cfg(feature = "metadata")]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PropertyConstraints {
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    pub format: Option<&'static str>,
    pub multiline: bool,
    pub recommended_rows: Option<u32>,
    pub role: Option<&'static str>,
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
    pub name: &'static str,
    pub description: &'static str,
    pub category: NodeCategory,
    pub inputs: &'static [InputPortDef],
    pub properties: Vec<PropertyDef>,
    pub default_properties: Vec<(&'static str, NodeProperty)>,
}

#[cfg(feature = "metadata")]
pub trait NodeSchema: Default {
    fn schema() -> NodeSchemaDef;
}

#[cfg(feature = "json")]
pub trait JsonNode: Default {
    fn from_json(
        id: NodeId,
        properties: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> anyhow::Result<Self>
    where
        Self: Sized;

    fn set_input_json(&mut self, port: &str, source: PortRef) -> anyhow::Result<()>;
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

#[derive(Debug, Clone)]
pub enum Deferred<T> {
    Value(T),
    Expr(Expression),
}

impl<T> Deferred<T> {
    pub fn value(value: T) -> Self {
        Self::Value(value)
    }

    pub fn eval(
        &self,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<T>
    where
        T: DeferredValue,
    {
        T::eval_deferred(self, node_id, property_path, ctx)
    }

    pub fn to_node_property(&self) -> NodeProperty
    where
        T: DeferredValue,
    {
        match self {
            Self::Value(value) => T::to_node_property(value),
            Self::Expr(expr) => NodeProperty::Expr(expr.clone()),
        }
    }
}

impl<T> From<T> for Deferred<T> {
    fn from(value: T) -> Self {
        Self::Value(value)
    }
}

impl Deferred<f64> {
    pub fn resolve_float(
        &self,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<f64> {
        self.eval(node_id, property_path, ctx)
    }
}

impl Deferred<i64> {
    pub fn resolve_int(
        &self,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<i64> {
        self.eval(node_id, property_path, ctx)
    }

    pub fn resolve_float(
        &self,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<f64> {
        self.eval(node_id, property_path, ctx)
            .map(|value| value as f64)
    }
}

impl Deferred<bool> {
    pub fn resolve_bool(
        &self,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<bool> {
        self.eval(node_id, property_path, ctx)
    }
}

impl Deferred<String> {
    pub fn resolve_string(
        &self,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<String> {
        self.eval(node_id, property_path, ctx)
    }
}

impl Deferred<[u8; 4]> {
    pub fn resolve_color(
        &self,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<[u8; 4]> {
        self.eval(node_id, property_path, ctx)
    }
}

impl Deferred<(f64, f64)> {
    pub fn resolve_vec2(
        &self,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<(f64, f64)> {
        self.eval(node_id, property_path, ctx)
    }
}

#[cfg(feature = "json")]
impl<'de, T> serde::Deserialize<'de> for Deferred<T>
where
    T: DeferredJsonValue,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        if let Some(text) = value.as_str()
            && let Some(source) = text.strip_prefix('=')
        {
            return Expression::parse(source)
                .map(Self::Expr)
                .map_err(serde::de::Error::custom);
        }
        T::from_json_value(&value)
            .map(Self::Value)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(feature = "json")]
pub trait DeferredJsonValue: Sized {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, String>;
}

pub trait DeferredValue: Clone {
    fn eval_deferred(
        deferred: &Deferred<Self>,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<Self>
    where
        Self: Sized;

    fn to_node_property(value: &Self) -> NodeProperty;
}

#[cfg(feature = "json")]
impl DeferredJsonValue for f64 {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, String> {
        value
            .as_f64()
            .or_else(|| value.as_i64().map(|value| value as f64))
            .ok_or_else(|| "expected float".to_string())
    }
}

impl DeferredValue for f64 {
    fn eval_deferred(
        deferred: &Deferred<Self>,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<Self> {
        match deferred {
            Deferred::Value(value) => Ok(*value),
            Deferred::Expr(expr) => expr.evaluate(ctx)?.as_f64().ok_or_else(|| {
                NodeProperty::invalid_type(node_id, property_path, "Float", "expression")
            }),
        }
    }

    fn to_node_property(value: &Self) -> NodeProperty {
        NodeProperty::Float(*value)
    }
}

#[cfg(feature = "json")]
impl DeferredJsonValue for i64 {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, String> {
        value
            .as_i64()
            .or_else(|| value.as_f64().map(|value| value as i64))
            .ok_or_else(|| "expected int".to_string())
    }
}

impl DeferredValue for i64 {
    fn eval_deferred(
        deferred: &Deferred<Self>,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<Self> {
        match deferred {
            Deferred::Value(value) => Ok(*value),
            Deferred::Expr(expr) => {
                let value = expr.evaluate(ctx)?.as_f64().ok_or_else(|| {
                    NodeProperty::invalid_type(node_id, property_path, "Int", "expression")
                })?;
                Ok(value as i64)
            }
        }
    }

    fn to_node_property(value: &Self) -> NodeProperty {
        NodeProperty::Int(*value)
    }
}

#[cfg(feature = "json")]
impl DeferredJsonValue for bool {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, String> {
        value
            .as_bool()
            .or_else(|| value.as_i64().map(|value| value != 0))
            .ok_or_else(|| "expected bool".to_string())
    }
}

impl DeferredValue for bool {
    fn eval_deferred(
        deferred: &Deferred<Self>,
        _node_id: NodeId,
        _property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<Self> {
        match deferred {
            Deferred::Value(value) => Ok(*value),
            Deferred::Expr(expr) => Ok(expr.evaluate(ctx)?.as_bool()),
        }
    }

    fn to_node_property(value: &Self) -> NodeProperty {
        NodeProperty::Bool(*value)
    }
}

#[cfg(feature = "json")]
impl DeferredJsonValue for String {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, String> {
        value
            .as_str()
            .map(ToString::to_string)
            .ok_or_else(|| "expected string".to_string())
    }
}

impl DeferredValue for String {
    fn eval_deferred(
        deferred: &Deferred<Self>,
        _node_id: NodeId,
        _property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<Self> {
        match deferred {
            Deferred::Value(value) => Ok(value.clone()),
            Deferred::Expr(expr) => Ok(expr.evaluate(ctx)?.as_string()),
        }
    }

    fn to_node_property(value: &Self) -> NodeProperty {
        NodeProperty::String(value.clone())
    }
}

#[cfg(feature = "json")]
impl DeferredJsonValue for [u8; 4] {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, String> {
        crate::json::parse_color(value).ok_or_else(|| "expected color".to_string())
    }
}

impl DeferredValue for [u8; 4] {
    fn eval_deferred(
        deferred: &Deferred<Self>,
        node_id: NodeId,
        property_path: &str,
        _ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<Self> {
        match deferred {
            Deferred::Value(value) => Ok(*value),
            Deferred::Expr(_) => Err(NodeProperty::invalid_type(
                node_id,
                property_path,
                "Color",
                "expression",
            )),
        }
    }

    fn to_node_property(value: &Self) -> NodeProperty {
        NodeProperty::Color(*value)
    }
}

#[cfg(feature = "json")]
impl DeferredJsonValue for (f64, f64) {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, String> {
        let values = value
            .as_array()
            .ok_or_else(|| "expected vec2".to_string())?;
        if values.len() != 2 {
            return Err(format!(
                "expected vec2, got array of length {}",
                values.len()
            ));
        }
        let x = values[0]
            .as_f64()
            .ok_or_else(|| "expected vec2 x number".to_string())?;
        let y = values[1]
            .as_f64()
            .ok_or_else(|| "expected vec2 y number".to_string())?;
        Ok((x, y))
    }
}

impl DeferredValue for (f64, f64) {
    fn eval_deferred(
        deferred: &Deferred<Self>,
        node_id: NodeId,
        property_path: &str,
        _ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<Self> {
        match deferred {
            Deferred::Value(value) => Ok(*value),
            Deferred::Expr(_) => Err(NodeProperty::invalid_type(
                node_id,
                property_path,
                "Vec2",
                "expression",
            )),
        }
    }

    fn to_node_property(value: &Self) -> NodeProperty {
        NodeProperty::Vec2(*value)
    }
}

pub struct NodeParamEvalContext<'a> {
    pub node_id: NodeId,
    pub expr: &'a crate::expr::ExpressionContext<'a>,
}

pub trait NodeParams: Clone + Default {
    type Evaluated;

    fn property_defs() -> Vec<PropertyDef>;
    fn is_property(id: &str) -> bool;
    fn default_properties(&self) -> Vec<(&'static str, NodeProperty)>;
    fn get_property(&self, id: &str) -> Option<NodeProperty>;
    fn eval(&self, ctx: &NodeParamEvalContext<'_>) -> crate::Result<Self::Evaluated>;

    #[cfg(feature = "json")]
    fn from_json(
        properties: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> anyhow::Result<Self>
    where
        Self: Sized,
        Self: serde::de::DeserializeOwned,
    {
        let value = properties
            .cloned()
            .map(serde_json::Value::Object)
            .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
        serde_path_to_error::deserialize(value).map_err(|error| anyhow::anyhow!(error.to_string()))
    }
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

    #[cfg(feature = "metadata")]
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
