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
    pub name: String,
    pub kind: NodePropertyKind,
    pub default_value: NodeLiteralValue,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeSpec {
    pub kind: String,
    pub label: String,
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
    write_if_changed(&out_dir.join("meta.json"), &artifacts.meta_json)?;
    write_if_changed(
        &out_dir.join("schemas/meta.schema.json"),
        &artifacts.meta_schema_json,
    )?;
    write_if_changed(
        &out_dir.join("schemas/composition.schema.json"),
        &artifacts.composition_schema_json,
    )?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct DefinitionArtifacts {
    pub manifest: MetaManifest,
    pub meta_json: String,
    pub meta_schema_json: String,
    pub composition_schema_json: String,
}

pub fn render_definition_artifacts() -> Result<DefinitionArtifacts> {
    let manifest = meta_manifest()?;
    Ok(DefinitionArtifacts {
        meta_json: render_meta_json(&manifest)?,
        meta_schema_json: render_meta_schema_json()?,
        composition_schema_json: render_composition_schema_json(&manifest)?,
        manifest,
    })
}

pub fn validate_generated_artifacts(
    meta_json: &str,
    meta_schema_json: &str,
    composition_schema_json: &str,
) -> Result<()> {
    let meta_json: Value = serde_json::from_str(meta_json)?;
    let meta_schema: Value = serde_json::from_str(meta_schema_json)?;
    let composition_schema: Value = serde_json::from_str(composition_schema_json)?;
    jsonschema::meta::validate(&meta_schema)
        .map_err(|error| anyhow!("generated meta schema is invalid: {error}"))?;
    jsonschema::meta::validate(&composition_schema)
        .map_err(|error| anyhow!("generated composition schema is invalid: {error}"))?;
    let meta_validator = jsonschema::validator_for(&meta_schema)?;
    if let Err(error) = meta_validator.validate(&meta_json) {
        bail!("generated meta json is invalid: {error}");
    }
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

pub fn render_meta_json(manifest: &MetaManifest) -> Result<String> {
    Ok(format!("{}\n", serde_json::to_string_pretty(manifest)?))
}

pub fn render_meta_schema_json() -> Result<String> {
    Ok(format!(
        "{}\n",
        serde_json::to_string_pretty(&json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "required": ["schemaVersion", "nodeKinds", "nodeSpecs"],
            "properties": {
                "schemaVersion": { "type": "integer" },
                "nodeKinds": { "type": "array", "items": { "type": "string" } },
                "nodeSpecs": { "type": "object" }
            }
        }))?
    ))
}

pub fn render_composition_schema_json(manifest: &MetaManifest) -> Result<String> {
    Ok(format!(
        "{}\n",
        serde_json::to_string_pretty(&json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "additionalProperties": true,
            "$defs": {
                "nodeKinds": manifest.node_kinds
            }
        }))?
    ))
}

fn spec_from_schema(schema: lumen::node::NodeSchemaDef) -> Result<NodeSpec> {
    let properties = schema
        .properties
        .iter()
        .map(|property| {
            let default_value = schema
                .default_properties
                .iter()
                .find(|(name, _)| *name == property.name)
                .map(|(_, value)| literal_from_node_property(value))
                .transpose()?
                .with_context(|| {
                    format!(
                        "missing default value for property `{}` on node `{}`",
                        property.name, schema.kind
                    )
                })?;
            Ok(NodePropertySpec {
                name: property.name.to_string(),
                kind: property_kind_from_schema(property.expected),
                default_value,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let default_properties = properties
        .iter()
        .map(|property| (property.name.clone(), property.default_value.clone()))
        .collect();
    Ok(NodeSpec {
        kind: schema.kind.to_string(),
        label: schema.label.to_string(),
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
    }
}

fn literal_from_node_property(property: &lumen::node::NodeProperty) -> Result<NodeLiteralValue> {
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
