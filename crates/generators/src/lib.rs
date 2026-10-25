// The composition schema is one large `json!` literal.
#![recursion_limit = "256"]

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Generate(CliArgs),
    Help,
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
pub enum NodeParamKind {
    Float,
    Int,
    Bool,
    String,
    Color,
    Paint,
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
    Paint(Value),
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
pub struct NodeParamSpec {
    pub id: String,
    pub name: String,
    pub kind: NodeParamKind,
    pub description: String,
    pub default_value: NodeLiteralValue,
    pub accepts_expressions: bool,
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
    pub params: Vec<NodeParamSpec>,
    pub default_params: BTreeMap<String, NodeLiteralValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaManifest {
    pub schema_version: u32,
    pub node_specs: BTreeMap<String, NodeSpec>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

pub fn parse_args_from<I>(args: I) -> Result<Command>
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
            "--help" | "-h" => return Ok(Command::Help),
            unknown => bail!("unknown argument: {unknown}"),
        }
    }

    Ok(Command::Generate(CliArgs { target, out_dir }))
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
    let composition_schema_json = render_composition_schema_json(&manifest)?;
    validate_generated_artifacts(&composition_schema_json)?;
    Ok(DefinitionArtifacts {
        composition_schema_json,
        manifest,
    })
}

fn validate_generated_artifacts(composition_schema_json: &str) -> Result<()> {
    let composition_schema: Value = serde_json::from_str(composition_schema_json)?;
    jsonschema::meta::validate(&composition_schema)
        .map_err(|error| anyhow!("generated composition schema is invalid: {error}"))?;
    Ok(())
}

pub fn meta_manifest() -> Result<MetaManifest> {
    let specs = lumen_engine::node::NodeKind::schemas()
        .into_iter()
        .map(spec_from_schema)
        .collect::<Result<Vec<_>>>()?;
    let node_specs = specs
        .into_iter()
        .map(|spec| (spec.kind.clone(), spec))
        .collect();
    Ok(MetaManifest {
        schema_version: META_SCHEMA_VERSION,
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
                    "required": ["duration_frames"],
                    "properties": {
                        "fps": { "type": "number", "exclusiveMinimum": 0, "default": 30 },
                        "duration_frames": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 4294967295u32
                        }
                    },
                    "additionalProperties": true
                },
                "audio": {
                    "type": "object",
                    "required": ["tracks", "clips"],
                    "properties": {
                        "tracks": {
                            "type": "array",
                            "items": { "$ref": "#/$defs/audioTrack" }
                        },
                        "clips": {
                            "type": "array",
                            "items": { "$ref": "#/$defs/audioClip" }
                        }
                    },
                    "additionalProperties": true
                },
                "render_settings": {
                    "type": "object",
                    "required": ["width", "height"],
                    "properties": {
                        "width": { "type": "integer", "minimum": 1, "maximum": 4294967295u32 },
                        "height": { "type": "integer", "minimum": 1, "maximum": 4294967295u32 },
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
                "expression": {
                    "description": "An expression evaluated per frame in place of a literal value.",
                    "type": "string",
                    "pattern": "^=([^=]|$)"
                },
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
                "paint": {
                    "oneOf": [
                        { "$ref": "#/$defs/color" },
                        {
                            "type": "object",
                            "required": ["type", "stops"],
                            "properties": {
                                "type": {
                                    "type": "string",
                                    "enum": ["linear", "linear_gradient", "radial", "radial_gradient", "conic", "conic_gradient"]
                                },
                                "units": {
                                    "type": "string",
                                    "enum": ["object_bounding_box", "user_space", "userSpaceOnUse"],
                                    "default": "object_bounding_box"
                                },
                                "spread": {
                                    "type": "string",
                                    "enum": ["pad", "repeat", "reflect"],
                                    "default": "pad"
                                },
                                "interpolation": {
                                    "type": "string",
                                    "enum": ["srgb", "linear_srgb", "linear"],
                                    "default": "srgb"
                                },
                                "start": { "$ref": "#/$defs/vec2" },
                                "end": { "$ref": "#/$defs/vec2" },
                                "center": { "$ref": "#/$defs/vec2" },
                                "radius": {
                                    "oneOf": [{ "type": "number" }, { "$ref": "#/$defs/vec2" }]
                                },
                                "angle": { "type": "number" },
                                "stops": {
                                    "type": "array",
                                    "minItems": 1,
                                    "items": {
                                        "oneOf": [
                                            {
                                                "type": "object",
                                                "required": ["offset", "color"],
                                                "properties": {
                                                    "offset": { "type": "number" },
                                                    "color": { "$ref": "#/$defs/color" }
                                                },
                                                "additionalProperties": false
                                            },
                                            {
                                                "type": "array",
                                                "prefixItems": [
                                                    { "type": "number" },
                                                    { "$ref": "#/$defs/color" }
                                                ],
                                                "minItems": 2,
                                                "maxItems": 2
                                            }
                                        ]
                                    }
                                }
                            },
                            "additionalProperties": false
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
                },
                "audioTrack": {
                    "type": "object",
                    "required": ["id"],
                    "properties": {
                        "id": { "type": "string" },
                        "name": { "type": "string" },
                        "muted": { "type": "boolean" },
                        "solo": { "type": "boolean" },
                        "volume": { "type": "number" }
                    },
                    "additionalProperties": true
                },
                "audioClip": {
                    "type": "object",
                    "required": ["id", "source_id", "track_id"],
                    "properties": {
                        "id": { "type": "string" },
                        "source_id": { "type": "string" },
                        "track_id": { "type": "string" },
                        "name": { "type": "string" },
                        "start_ms": { "type": "integer", "minimum": 0 },
                        "duration_ms": { "type": "integer", "minimum": 1 },
                        "source_start_ms": { "type": "integer", "minimum": 0 },
                        "start_frame": { "type": "integer", "minimum": 0 },
                        "duration_frames": { "type": "integer", "minimum": 1 },
                        "source_start_seconds": { "type": "number", "minimum": 0 },
                        "volume": { "type": "number" }
                    },
                    "additionalProperties": true
                }
            }
        }))?
    ))
}

fn node_schema_json(spec: &NodeSpec) -> Value {
    let mut properties = serde_json::Map::new();
    for property in &spec.params {
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
            "params": {
                "type": "object",
                "properties": properties,
                "additionalProperties": false
            }
        },
        "additionalProperties": false
    })
}

fn property_schema_json(property: &NodeParamSpec) -> Value {
    let value_schema = match property.kind {
        NodeParamKind::Float => numeric_property_schema_json("number", property),
        NodeParamKind::Int => numeric_property_schema_json("integer", property),
        NodeParamKind::Bool => json!({ "type": "boolean" }),
        // A single leading `=` parses as an expression; `==` escapes a literal `=`.
        NodeParamKind::String if !property.accepts_expressions => {
            json!({ "type": "string", "not": { "pattern": "^=([^=]|$)" } })
        }
        NodeParamKind::String => json!({ "type": "string" }),
        NodeParamKind::Color => json!({ "$ref": "#/$defs/color" }),
        NodeParamKind::Paint => json!({ "$ref": "#/$defs/paint" }),
        NodeParamKind::Vec2 => json!({ "$ref": "#/$defs/vec2" }),
        NodeParamKind::Enum => {
            let values = property
                .enum_options
                .iter()
                .map(|option| option.name.clone())
                .collect::<Vec<_>>();
            json!({ "type": "string", "enum": values })
        }
    };
    let mut schema = if property.accepts_expressions {
        json!({ "anyOf": [value_schema, { "$ref": "#/$defs/expression" }] })
    } else {
        value_schema
    };
    if let Value::Object(object) = &mut schema {
        object.insert("title".to_string(), json!(property.name));
        object.insert("description".to_string(), json!(property.description));
        object.insert("x-lumen-kind".to_string(), json!(property.kind));
        object.insert("default".to_string(), json!(property.default_value));
        if !property.enum_options.is_empty() {
            object.insert(
                "x-lumen-enumOptions".to_string(),
                json!(property.enum_options),
            );
        }
        if !property.constraints.is_empty() {
            object.insert(
                "x-lumen-constraints".to_string(),
                json!(property.constraints),
            );
        }
    }
    schema
}

fn numeric_property_schema_json(kind: &str, property: &NodeParamSpec) -> Value {
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

fn spec_from_schema(schema: lumen_engine::node::NodeSchemaDef) -> Result<NodeSpec> {
    let params = schema
        .properties
        .iter()
        .map(|property| {
            let raw_default_value = schema
                .default_properties
                .iter()
                .find(|(name, _)| *name == property.id)
                .map(|(_, value)| literal_from_property_value(value, property.enum_def))
                .transpose()?
                .with_context(|| {
                    format!(
                        "missing default value for param `{}` on node `{}`",
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
            Ok(NodeParamSpec {
                id: property.id.to_string(),
                name: property.name.to_string(),
                kind: property_kind_from_schema(property.expected),
                description: property.description.to_string(),
                default_value: raw_default_value,
                accepts_expressions: property.accepts_expressions,
                enum_options,
                constraints: constraints_from_schema(property.constraints),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let default_params = params
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
        outputs: lumen_engine::node::SINGLE_RASTER_OUTPUT
            .iter()
            .map(|output| NodeOutputPortSpec {
                name: output.name.to_string(),
                kind: port_kind_from_schema(output.kind),
            })
            .collect(),
        params,
        default_params,
    })
}

fn category_from_schema(category: lumen_engine::node::NodeCategory) -> NodeCategory {
    match category {
        lumen_engine::node::NodeCategory::Compositing => NodeCategory::Compositing,
        lumen_engine::node::NodeCategory::Processing => NodeCategory::Processing,
        lumen_engine::node::NodeCategory::Source => NodeCategory::Source,
        lumen_engine::node::NodeCategory::Output => NodeCategory::Output,
        lumen_engine::node::NodeCategory::Vector => NodeCategory::Vector,
    }
}

fn port_kind_from_schema(kind: lumen_engine::node::PortKind) -> NodePortKind {
    match kind {
        lumen_engine::node::PortKind::Raster => NodePortKind::RasterFrame,
        lumen_engine::node::PortKind::Vector => NodePortKind::Vector,
    }
}

fn property_kind_from_schema(kind: lumen_engine::node::PropertyKind) -> NodeParamKind {
    match kind {
        lumen_engine::node::PropertyKind::Float => NodeParamKind::Float,
        lumen_engine::node::PropertyKind::Int => NodeParamKind::Int,
        lumen_engine::node::PropertyKind::Bool => NodeParamKind::Bool,
        lumen_engine::node::PropertyKind::String => NodeParamKind::String,
        lumen_engine::node::PropertyKind::Color => NodeParamKind::Color,
        lumen_engine::node::PropertyKind::Paint => NodeParamKind::Paint,
        lumen_engine::node::PropertyKind::Vec2 => NodeParamKind::Vec2,
        lumen_engine::node::PropertyKind::Enum => NodeParamKind::Enum,
    }
}

fn constraints_from_schema(
    constraints: lumen_engine::node::PropertyConstraints,
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

fn literal_from_property_value(
    property: &lumen_engine::node::PropertyValue,
    enum_def: Option<&'static lumen_engine::node::EnumDef>,
) -> Result<NodeLiteralValue> {
    if let Some(enum_def) = enum_def {
        let lumen_engine::node::PropertyValue::Int(value) = property else {
            bail!("enum default param must be stored as an int: {property:?}");
        };
        let option = enum_def
            .options
            .iter()
            .find(|option| option.value == *value)
            .with_context(|| format!("enum default `{value}` not found in `{}`", enum_def.name))?;
        return Ok(NodeLiteralValue::String(option.name.to_string()));
    }

    match property {
        lumen_engine::node::PropertyValue::Float(value) => Ok(NodeLiteralValue::Float(*value)),
        lumen_engine::node::PropertyValue::Int(value) => Ok(NodeLiteralValue::Int(*value)),
        lumen_engine::node::PropertyValue::Bool(value) => Ok(NodeLiteralValue::Bool(*value)),
        lumen_engine::node::PropertyValue::String(value) => {
            Ok(NodeLiteralValue::String(value.clone()))
        }
        lumen_engine::node::PropertyValue::Color(value) => Ok(NodeLiteralValue::Color(*value)),
        lumen_engine::node::PropertyValue::Paint(value) => {
            Ok(NodeLiteralValue::Paint(value.to_json_value()))
        }
        lumen_engine::node::PropertyValue::Vec2(value) => Ok(NodeLiteralValue::Vec2(*value)),
        other => bail!("unsupported default param literal: {other:?}"),
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

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE_ID: i64 = 100;
    const OUTPUT_ID: i64 = 101;

    fn composition(node: Value, connections: Vec<Value>, background_color: Value) -> Value {
        json!({
            "timeline": { "fps": 30, "duration_frames": 10 },
            "render_settings": { "width": 16, "height": 16, "background_color": background_color },
            "nodes": [
                node,
                { "id": SOURCE_ID, "type": "background" },
                { "id": OUTPUT_ID, "type": "media_output" }
            ],
            "connections": connections
        })
    }

    /// A renderable composition containing `spec` with `params`, its required inputs fed by a
    /// background node, and its output wired to the media output.
    fn composition_with_node(spec: &NodeSpec, params: Value) -> Value {
        let mut connections = spec
            .inputs
            .iter()
            .filter(|input| !input.optional)
            .map(|input| json!({ "from_node": SOURCE_ID, "to_node": 1, "to_port": input.name }))
            .collect::<Vec<_>>();
        connections.push(json!({ "from_node": 1, "to_node": OUTPUT_ID, "to_port": "source" }));
        composition(
            json!({ "id": 1, "type": spec.kind, "params": params }),
            connections,
            json!([0, 0, 0, 255]),
        )
    }

    fn schema_validator() -> jsonschema::Validator {
        let artifacts = render_definition_artifacts().unwrap();
        let schema: Value = serde_json::from_str(&artifacts.composition_schema_json).unwrap();
        jsonschema::validator_for(&schema).unwrap()
    }

    #[test]
    fn expression_support_matches_engine_parsing() {
        let manifest = meta_manifest().unwrap();
        let validator = schema_validator();
        for spec in manifest.node_specs.values() {
            for param in &spec.params {
                let label = format!("{}.{}", spec.kind, param.id);
                let literal = composition_with_node(
                    spec,
                    json!({ param.id.clone(): param.default_value.clone() }),
                );
                lumen_engine::json::parse_value(&literal)
                    .unwrap_or_else(|error| panic!("{label} literal should parse: {error:#}"));
                assert!(
                    validator.is_valid(&literal),
                    "{label} literal should validate"
                );

                let expression = composition_with_node(spec, json!({ param.id.clone(): "=1" }));
                let engine_accepts = lumen_engine::json::parse_value(&expression).is_ok();
                assert_eq!(engine_accepts, param.accepts_expressions, "{label} engine");
                assert_eq!(
                    validator.is_valid(&expression),
                    param.accepts_expressions,
                    "{label} schema"
                );
            }
        }
    }

    #[test]
    fn timeline_schema_matches_engine_parsing() {
        let manifest = meta_manifest().unwrap();
        let validator = schema_validator();
        let with_timeline = |timeline: Value| {
            let mut value = composition_with_node(&manifest.node_specs["exposure"], json!({}));
            value["timeline"] = timeline;
            value
        };

        for (timeline, valid) in [
            (json!({ "duration_frames": 10 }), true),
            (json!({ "fps": 24, "duration_frames": 10.0 }), true),
            (json!({ "fps": 0, "duration_frames": 10 }), false),
            (json!({ "fps": -30, "duration_frames": 10 }), false),
            (json!({ "fps": 30, "duration_frames": 0 }), false),
            (
                json!({ "fps": 30, "duration_frames": 4294967296u64 }),
                false,
            ),
        ] {
            let composition = with_timeline(timeline.clone());
            assert_eq!(validator.is_valid(&composition), valid, "{timeline}");
            assert_eq!(
                lumen_engine::json::parse_value(&composition).is_ok(),
                valid,
                "{timeline}"
            );
        }
    }

    #[test]
    fn schema_accepts_expressions_only_for_expression_params() {
        let manifest = meta_manifest().unwrap();
        let validator = schema_validator();
        let node =
            |kind: &str, params: Value| composition_with_node(&manifest.node_specs[kind], params);

        assert!(validator.is_valid(&node("exposure", json!({ "exposure": "=frame * 0.1" }))));
        assert!(validator.is_valid(&node("text", json!({ "position": "=vec2(frame, 0)" }))));
        // `media_in.kind` is an `i64`-backed enum, so it evaluates expressions.
        assert!(validator.is_valid(&node("media_in", json!({ "kind": "=1" }))));

        assert!(!validator.is_valid(&node("exposure", json!({ "exposure": "frame" }))));
        assert!(!validator.is_valid(&node("text", json!({ "color": "=frame" }))));
        assert!(!validator.is_valid(&node("text", json!({ "position": ["=frame", 0] }))));
        assert!(!validator.is_valid(&node("transform", json!({ "sampling": "=1" }))));
        // `alpha_premultiply.mode` decides output metadata, so it is literal-only.
        let mode = manifest.node_specs["alpha_premultiply"]
            .params
            .iter()
            .find(|param| param.id == "mode")
            .unwrap();
        assert!(!mode.accepts_expressions);
        let expression_mode = node("alpha_premultiply", json!({ "mode": "='premultiply'" }));
        assert!(!validator.is_valid(&expression_mode));
        assert!(lumen_engine::json::parse_value(&expression_mode).is_err());
        let escaped_mode = node("alpha_premultiply", json!({ "mode": "==premultiply" }));
        assert!(validator.is_valid(&escaped_mode));
        assert!(lumen_engine::json::parse_value(&escaped_mode).is_ok());
        let gradient_with_expression_stop = json!({
            "type": "linear",
            "stops": [{ "offset": 0, "color": "=frame" }]
        });
        assert!(!validator.is_valid(&node(
            "text",
            json!({ "color": gradient_with_expression_stop })
        )));
        assert!(!validator.is_valid(&composition(
            json!({ "id": 1, "type": "background" }),
            vec![json!({ "from_node": 1, "to_node": OUTPUT_ID, "to_port": "source" })],
            json!("=frame"),
        )));
    }
}
