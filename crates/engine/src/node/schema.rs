use super::ids::NodeId;
use super::media_source::MediaParam;
use super::ports::{InputPortDef, PortKind};

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
    Paint = 7,
    ShaderUniforms = 8,
    ClipProps = 9,
    TextContent = 10,
    TextLine = 11,
    TextHighlight = 12,
    FontFeatures = 13,
    TextStroke = 14,
    DashPattern = 15,
    Repeat = 16,
    Vec3 = 17,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnumOptionDef {
    pub name: &'static str,
    pub label: &'static str,
    pub value: i64,
    /// Other names JSON accepts for the option, from its `#[serde(alias)]` attributes.
    pub aliases: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnumDef {
    pub name: &'static str,
    pub options: &'static [EnumOptionDef],
}

pub trait NodeEnum {
    fn enum_def() -> &'static EnumDef;
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PropertyDef {
    pub id: &'static str,
    pub expected: PropertyKind,
    pub enum_def: Option<&'static EnumDef>,
    /// What `#[media(...)]` declares, for a param that names media.
    pub media: Option<MediaParam>,
    #[cfg(feature = "metadata")]
    pub name: &'static str,
    #[cfg(feature = "metadata")]
    pub description: &'static str,
    #[cfg(feature = "metadata")]
    pub constraints: PropertyConstraints,
    /// Whether the param accepts an `"=..."` expression in place of a literal.
    #[cfg(feature = "metadata")]
    pub accepts_expressions: bool,
    /// Whether JSON `null` is a value of its own, as for `Override` fields.
    #[cfg(feature = "metadata")]
    pub nullable: bool,
    /// Whether a nested object must set the field. Applies to fields without a default.
    #[cfg(feature = "metadata")]
    pub required: bool,
}

#[cfg(feature = "metadata")]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PropertyConstraints {
    pub min: Option<f64>,
    /// Whether the engine refuses literals below `min`, which the schema then does too. Other
    /// ranges are editor hints.
    pub enforced_min: bool,
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
    Scene = 5,
}

#[derive(Debug, Clone)]
pub struct NodeSchemaDef {
    pub kind: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub category: NodeCategory,
    pub inputs: &'static [InputPortDef],
    pub output: PortKind,
    pub properties: Vec<PropertyDef>,
    pub default_properties: Vec<(&'static str, serde_json::Value)>,
}

#[cfg(feature = "metadata")]
pub trait NodeSchema: Default {
    fn schema() -> NodeSchemaDef;
}

pub trait JsonNode: Default {
    /// The node's `type` in JSON.
    const KIND: &'static str;

    fn from_json(
        id: NodeId,
        params: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> Result<Self, Vec<crate::error::DocumentError>>
    where
        Self: Sized;

    fn set_input_json(
        &mut self,
        port: &str,
        source: NodeId,
    ) -> Result<(), crate::error::GraphValidationError>;
}
