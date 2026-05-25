use super::{
    ids::NodeId,
    ports::{InputPortDef, PortRef},
    property::PropertyValue,
};

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
    pub default_properties: Vec<(&'static str, PropertyValue)>,
}

#[cfg(feature = "metadata")]
pub trait NodeSchema: Default {
    fn schema() -> NodeSchemaDef;
}

#[cfg(feature = "json")]
pub trait JsonNode: Default {
    fn from_json(
        id: NodeId,
        params: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> anyhow::Result<Self>
    where
        Self: Sized;

    fn set_input_json(&mut self, port: &str, source: PortRef) -> anyhow::Result<()>;
}
