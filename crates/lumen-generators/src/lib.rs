use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use serde_json::{Value, json};

pub const DEFAULT_DEFINITIONS_OUTPUT_DIR: &str = "definitions";
pub const META_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliArgs {
    pub target: GenerateTarget,
    pub out_dir: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerateTarget {
    Definitions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeCategory {
    Compositing,
    Processing,
    Source,
    Output,
    Vector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodePortKind {
    RasterFrame,
    Vector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodePropertyKind {
    Float,
    Int,
    Bool,
    String,
    Color,
    Vec2,
    Enum,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum NodeLiteralValue {
    Float(f64),
    Int(i64),
    Bool(bool),
    String(String),
    Color([u8; 4]),
    Vec2((f64, f64)),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeInputPortSpec {
    pub name: String,
    pub kind: NodePortKind,
    pub optional: bool,
    pub variadic: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeOutputPortSpec {
    pub name: String,
    pub kind: NodePortKind,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodePropertySpec {
    pub id: String,
    pub name: String,
    pub kind: NodePropertyKind,
    pub description: String,
    pub default_value: NodeLiteralValue,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub enum_options: Vec<NodeEnumOptionSpec>,
    #[serde(skip_serializing_if = "PropertyConstraintsSpec::is_empty")]
    pub constraints: PropertyConstraintsSpec,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PropertyConstraintsSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub multiline: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recommended_rows: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

impl PropertyConstraintsSpec {
    fn is_empty(&self) -> bool {
        self.min.is_none()
            && self.max.is_none()
            && self.step.is_none()
            && self.format.is_none()
            && !self.multiline
            && self.recommended_rows.is_none()
            && self.role.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeEnumOptionSpec {
    pub name: String,
    pub label: String,
    pub value: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeSpec {
    pub kind: String,
    pub name: String,
    pub description: String,
    pub category: NodeCategory,
    pub inputs: Vec<NodeInputPortSpec>,
    pub outputs: Vec<NodeOutputPortSpec>,
    pub properties: Vec<NodePropertySpec>,
    pub default_properties: BTreeMap<String, NodeLiteralValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaManifest {
    pub schema_version: u32,
    pub node_kinds: Vec<String>,
    pub node_specs: BTreeMap<String, NodeSpec>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

pub fn parse_args_from<I>(args: I) -> Result<CliArgs>
where
    I: IntoIterator<Item = String>,
{
    let mut out_dir = PathBuf::from(DEFAULT_DEFINITIONS_OUTPUT_DIR);
    let target = GenerateTarget::Definitions;

    let mut args = args.into_iter().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "definitions" => {}
            "--out-dir" => {
                out_dir = PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow!("missing value for --out-dir"))?,
                );
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            unknown => bail!("unknown argument: {unknown}"),
        }
    }

    Ok(CliArgs { target, out_dir })
}

pub fn print_usage() {
    println!("Usage: cargo run -p lumen-generators -- [definitions] [--out-dir <path>]");
}

pub fn generate(args: &CliArgs) -> Result<()> {
    match args.target {
        GenerateTarget::Definitions => generate_definitions(&args.out_dir),
    }
}

pub fn generate_definitions(out_dir: &Path) -> Result<()> {
    let artifacts = render_definition_artifacts()?;
    fs::create_dir_all(out_dir)
        .with_context(|| format!("failed to create output directory `{}`", out_dir.display()))?;
    write_if_changed(
        &out_dir.join("composition.schema.json"),
        &artifacts.composition_schema_json,
    )?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct DefinitionArtifacts {
    pub manifest: MetaManifest,
    pub composition_schema_json: String,
}

pub fn render_definition_artifacts() -> Result<DefinitionArtifacts> {
    let manifest = meta_manifest()?;
    Ok(DefinitionArtifacts {
        composition_schema_json: render_composition_schema_json(&manifest)?,
        manifest,
    })
}

pub fn validate_generated_artifacts(
    composition_schema_json: &str,
) -> Result<()> {
    let composition_schema: Value = serde_json::from_str(composition_schema_json)?;
    jsonschema::meta::validate(&composition_schema)
        .map_err(|error| anyhow!("generated composition schema is invalid: {error}"))?;
    Ok(())
}

pub fn meta_manifest() -> Result<MetaManifest> {
    let specs = lumen::node::NodeKind::schemas()
        .into_iter()
        .map(spec_from_schema)
        .collect::<Result<Vec<_>>>()?;
    let node_kinds = specs.iter().map(|spec| spec.kind.clone()).collect();
    let node_specs = specs
        .into_iter()
        .map(|spec| (spec.kind.clone(), spec))
        .collect();
    Ok(MetaManifest {
        schema_version: META_SCHEMA_VERSION,
        node_kinds,
        node_specs,
    })
}

pub fn render_composition_schema_json(manifest: &MetaManifest) -> Result<String> {
    let node_variants = manifest
        .node_specs
        .values()
        .map(node_schema_json)
        .collect::<Vec<_>>();
    Ok(format!(
        "{}\n",
        serde_json::to_string_pretty(&json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "x-lumen-schemaVersion": manifest.schema_version,
            "type": "object",
            "required": ["timeline", "render_settings", "nodes", "connections"],
            "additionalProperties": true,
            "properties": {
                "timeline": {
                    "type": "object",
                    "required": ["fps", "duration_frames"],
                    "properties": {
                        "fps": { "type": "number" },
                        "duration_frames": { "type": "integer", "minimum": 1 }
                    },
                    "additionalProperties": true
                },
                "render_settings": {
                    "type": "object",
                    "required": ["width", "height"],
                    "properties": {
                        "width": { "type": "integer", "minimum": 1 },
                        "height": { "type": "integer", "minimum": 1 },
                        "background_color": { "$ref": "#/$defs/color" }
                    },
                    "additionalProperties": true
                },
                "nodes": {
                    "type": "array",
                    "items": { "oneOf": node_variants }
                },
                "connections": {
                    "type": "array",
                    "items": { "$ref": "#/$defs/connection" }
                }
            },
            "$defs": {
                "nodeKinds": manifest.node_kinds,
                "color": {
                    "oneOf": [
                        {
                            "type": "array",
                            "minItems": 3,
                            "maxItems": 4,
                            "items": { "type": "integer", "minimum": 0, "maximum": 255 }
                        },
                        {
                            "type": "string",
                            "pattern": "^#([0-9a-fA-F]{6}|[0-9a-fA-F]{8})$"
                        }
                    ]
                },
                "vec2": {
                    "type": "array",
                    "prefixItems": [{ "type": "number" }, { "type": "number" }],
                    "items": false,
                    "minItems": 2,
                    "maxItems": 2
                },
                "connection": {
                    "type": "object",
                    "required": ["from_node", "to_node", "to_port"],
                    "properties": {
                        "from_node": { "type": "integer" },
                        "from_port": { "type": "string" },
                        "to_node": { "type": "integer" },
                        "to_port": { "type": "string" }
                    },
                    "additionalProperties": false
                }
            }
        }))?
    ))
}

fn node_schema_json(spec: &NodeSpec) -> Value {
    let mut properties = serde_json::Map::new();
    for property in &spec.properties {
        properties.insert(property.id.clone(), property_schema_json(property));
    }
    json!({
        "title": spec.name,
        "description": spec.description,
        "x-lumen-category": spec.category,
        "x-lumen-inputs": spec.inputs,
        "x-lumen-outputs": spec.outputs,
        "type": "object",
        "required": ["id", "type"],
        "properties": {
            "id": { "type": "integer" },
            "type": { "const": spec.kind },
            "properties": {
                "type": "object",
                "properties": properties,
                "additionalProperties": false
            }
        },
        "additionalProperties": false
    })
}

fn property_schema_json(property: &NodePropertySpec) -> Value {
    let mut schema = match property.kind {
        NodePropertyKind::Float => numeric_property_schema_json("number", property),
        NodePropertyKind::Int => numeric_property_schema_json("integer", property),
        NodePropertyKind::Bool => json!({ "type": "boolean" }),
        NodePropertyKind::String => json!({ "type": "string" }),
        NodePropertyKind::Color => json!({ "$ref": "#/$defs/color" }),
        NodePropertyKind::Vec2 => json!({ "$ref": "#/$defs/vec2" }),
        NodePropertyKind::Enum => {
            let values = property
                .enum_options
                .iter()
                .map(|option| option.name.clone())
                .collect::<Vec<_>>();
            json!({ "type": "string", "enum": values })
        }
    };
    if let Value::Object(object) = &mut schema {
        object.insert("title".to_string(), json!(property.name));
        object.insert("description".to_string(), json!(property.description));
        object.insert("x-lumen-kind".to_string(), json!(property.kind));
        object.insert("default".to_string(), json!(property.default_value));
        if !property.enum_options.is_empty() {
            object.insert("x-lumen-enumOptions".to_string(), json!(property.enum_options));
        }
        if !property.constraints.is_empty() {
            object.insert("x-lumen-constraints".to_string(), json!(property.constraints));
        }
    }
    schema
}

fn numeric_property_schema_json(kind: &str, property: &NodePropertySpec) -> Value {
    let mut schema = serde_json::Map::new();
    schema.insert("type".to_string(), json!(kind));
    if let Some(min) = property.constraints.min {
        schema.insert("minimum".to_string(), json!(min));
    }
    if let Some(max) = property.constraints.max {
        schema.insert("maximum".to_string(), json!(max));
    }
    json!(schema)
}

fn spec_from_schema(schema: lumen::node::NodeSchemaDef) -> Result<NodeSpec> {
    let properties = schema
        .properties
        .iter()
        .map(|property| {
            let raw_default_value = schema
                .default_properties
                .iter()
                .find(|(name, _)| *name == property.id)
                .map(|(_, value)| literal_from_node_property(value, property.enum_def))
                .transpose()?
                .with_context(|| {
                    format!(
                        "missing default value for property `{}` on node `{}`",
                        property.id, schema.kind
                    )
                })?;
            let enum_options = property
                .enum_def
                .map(|enum_def| {
                    enum_def
                        .options
                        .iter()
                        .map(|option| NodeEnumOptionSpec {
                            name: option.name.to_string(),
                            label: option.label.to_string(),
                            value: option.value,
                        })
                        .collect()
                })
                .unwrap_or_default();
            Ok(NodePropertySpec {
                id: property.id.to_string(),
                name: property.name.to_string(),
                kind: property_kind_from_schema(property.expected),
                description: property.description.to_string(),
                default_value: raw_default_value,
                enum_options,
                constraints: constraints_from_schema(property.constraints),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let default_properties = properties
        .iter()
        .map(|property| (property.id.clone(), property.default_value.clone()))
        .collect();
    Ok(NodeSpec {
        kind: schema.kind.to_string(),
        name: schema.name.to_string(),
        description: schema.description.to_string(),
        category: category_from_schema(schema.category),
        inputs: schema
            .inputs
            .iter()
            .map(|input| NodeInputPortSpec {
                name: input.name.to_string(),
                kind: port_kind_from_schema(input.kind),
                optional: input.optional,
                variadic: input.variadic,
            })
            .collect(),
        outputs: vec![NodeOutputPortSpec {
            name: "output".to_string(),
            kind: NodePortKind::RasterFrame,
        }],
        properties,
        default_properties,
    })
}

fn category_from_schema(category: lumen::node::NodeCategory) -> NodeCategory {
    match category {
        lumen::node::NodeCategory::Compositing => NodeCategory::Compositing,
        lumen::node::NodeCategory::Processing => NodeCategory::Processing,
        lumen::node::NodeCategory::Source => NodeCategory::Source,
        lumen::node::NodeCategory::Output => NodeCategory::Output,
        lumen::node::NodeCategory::Vector => NodeCategory::Vector,
    }
}

fn port_kind_from_schema(kind: lumen::node::PortKind) -> NodePortKind {
    match kind {
        lumen::node::PortKind::Raster => NodePortKind::RasterFrame,
        lumen::node::PortKind::Vector => NodePortKind::Vector,
    }
}

fn property_kind_from_schema(kind: lumen::node::PropertyKind) -> NodePropertyKind {
    match kind {
        lumen::node::PropertyKind::Float => NodePropertyKind::Float,
        lumen::node::PropertyKind::Int => NodePropertyKind::Int,
        lumen::node::PropertyKind::Bool => NodePropertyKind::Bool,
        lumen::node::PropertyKind::String => NodePropertyKind::String,
        lumen::node::PropertyKind::Color => NodePropertyKind::Color,
        lumen::node::PropertyKind::Vec2 => NodePropertyKind::Vec2,
        lumen::node::PropertyKind::Enum => NodePropertyKind::Enum,
    }
}

fn constraints_from_schema(
    constraints: lumen::node::PropertyConstraints,
) -> PropertyConstraintsSpec {
    PropertyConstraintsSpec {
        min: constraints.min,
        max: constraints.max,
        step: constraints.step,
        format: constraints.format.map(String::from),
        multiline: constraints.multiline,
        recommended_rows: constraints.recommended_rows,
        role: constraints.role.map(String::from),
    }
}

fn literal_from_node_property(
    property: &lumen::node::NodeProperty,
    enum_def: Option<&'static lumen::node::EnumDef>,
) -> Result<NodeLiteralValue> {
    if let Some(enum_def) = enum_def {
        let lumen::node::NodeProperty::Int(value) = property else {
            bail!("enum default property must be stored as an int: {property:?}");
        };
        let option = enum_def
            .options
            .iter()
            .find(|option| option.value == *value)
            .with_context(|| format!("enum default `{value}` not found in `{}`", enum_def.name))?;
        return Ok(NodeLiteralValue::String(option.name.to_string()));
    }

    match property {
        lumen::node::NodeProperty::Float(value) => Ok(NodeLiteralValue::Float(*value)),
        lumen::node::NodeProperty::Int(value) => Ok(NodeLiteralValue::Int(*value)),
        lumen::node::NodeProperty::Bool(value) => Ok(NodeLiteralValue::Bool(*value)),
        lumen::node::NodeProperty::String(value) => Ok(NodeLiteralValue::String(value.clone())),
        lumen::node::NodeProperty::Color(value) => Ok(NodeLiteralValue::Color(*value)),
        lumen::node::NodeProperty::Vec2(value) => Ok(NodeLiteralValue::Vec2(*value)),
        other => bail!("unsupported default property literal: {other:?}"),
    }
}

fn write_if_changed(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create `{}`", parent.display()))?;
    }
    if matches!(fs::read_to_string(path), Ok(existing) if existing == content) {
        return Ok(());
    }
    fs::write(path, content).with_context(|| format!("failed to write `{}`", path.display()))
}
