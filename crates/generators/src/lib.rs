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
    ShaderUniforms,
    ClipProps,
    TextContent,
    TextLine,
    TextHighlight,
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
    ShaderUniforms(Value),
    ClipProps(Value),
    TextContent(Value),
    TextLine(Value),
    TextHighlight(Value),
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
    /// Absent for optional fields of a param kind, which take their parent's value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_value: Option<NodeLiteralValue>,
    pub accepts_expressions: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub nullable: bool,
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
    let mut schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "x-lumen-schemaVersion": manifest.schema_version,
        "x-lumen-expressions": expressions_json(),
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
            "functions": {
                "description": "Expression functions keyed by signature, such as `rise(start)`. Calls expand where they're used.",
                "type": "object",
                "propertyNames": {
                    "pattern": "^\\s*[A-Za-z_][A-Za-z0-9_]*\\s*\\(\\s*([A-Za-z_][A-Za-z0-9_]*\\s*(,\\s*[A-Za-z_][A-Za-z0-9_]*\\s*)*)?\\)\\s*$"
                },
                "additionalProperties": { "type": "string" }
            },
            "compositions": {
                "description": "Reusable compositions by name, which `clip` nodes play. Node ids are scoped to each template.",
                "type": "object",
                "additionalProperties": { "$ref": "#/$defs/compositionTemplate" }
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
            "compositionTemplate": {
                "description": "A composition a `clip` plays. Without `render_settings` it takes the root's size, and without `timeline` the root's timeline; a `timeline` without `fps` takes the root's fps.",
                "type": "object",
                "required": ["nodes", "connections"],
                "properties": {
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
                    "timeline": {
                        "type": "object",
                        "required": ["duration_frames"],
                        "properties": {
                            "fps": { "type": "number", "exclusiveMinimum": 0 },
                            "duration_frames": {
                                "type": "integer",
                                "minimum": 1,
                                "maximum": 4294967295u32
                            }
                        },
                        "additionalProperties": true
                    },
                    "props": {
                        "description": "Props that `prop('name')` reads inside the template, set per `clip`.",
                        "type": "object",
                        "additionalProperties": { "$ref": "#/$defs/compositionProp" }
                    },
                    "nodes": { "type": "array", "items": { "$ref": "#/properties/nodes/items" } },
                    "connections": {
                        "type": "array",
                        "items": { "$ref": "#/$defs/connection" }
                    }
                },
                "additionalProperties": false
            },
            "compositionProp": {
                "description": "A template prop's type and the value clips that don't set it get. Without a default it is empty, 0, transparent or [0, 0].",
                "oneOf": [
                    prop_declaration_json("string", json!({ "type": "string" })),
                    prop_declaration_json("number", json!({ "type": "number" })),
                    prop_declaration_json("color", json!({ "$ref": "#/$defs/color" })),
                    prop_declaration_json("vec2", json!({ "$ref": "#/$defs/vec2" }))
                ]
            },
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
                    "volume": { "type": "number", "minimum": 0, "maximum": f32::MAX as f64 }
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
                    "volume": { "type": "number", "minimum": 0, "maximum": f32::MAX as f64 }
                },
                "additionalProperties": true
            }
        }
    });
    schema["$defs"]
        .as_object_mut()
        .expect("`$defs` is an object")
        .extend(kind_defs_json()?);
    Ok(format!("{}\n", serde_json::to_string_pretty(&schema)?))
}

/// `$defs` of the param kinds whose fields are a delegate's `property_defs()`, and the outer
/// shapes around them that the property metadata can't express. Each carries the TypeScript
/// type name `x-lumen-type`.
fn kind_defs_json() -> Result<serde_json::Map<String, Value>> {
    use lumen_engine::node::{
        source::text_content::{TextHighlightDelegate, TextLineDelegate, TextSpanDelegate},
        vector::paint::{GradientPaintDelegate, GradientStopDelegate},
    };

    let mut defs = serde_json::Map::new();
    insert_object_def::<TextSpanDelegate>(&mut defs, "TextSpan", &["text"], &[])?;
    insert_object_def::<TextLineDelegate>(&mut defs, "TextLine", &[], &[])?;
    insert_object_def::<TextHighlightDelegate>(&mut defs, "TextHighlight", &[], &[])?;
    insert_def(
        &mut defs,
        "TextContent",
        json!({
            "description": "Text to render: a string, or spans that each override the node's text style.",
            "oneOf": [
                { "type": "string" },
                { "type": "array", "items": { "$ref": "#/$defs/textSpan" } }
            ]
        }),
    );

    insert_def(
        &mut defs,
        "Paint",
        json!({
            "description": "A solid color or a gradient.",
            "oneOf": [{ "$ref": "#/$defs/color" }, { "$ref": "#/$defs/gradientPaint" }]
        }),
    );
    insert_object_def::<GradientPaintDelegate>(
        &mut defs,
        "GradientPaint",
        &["type", "stops"],
        &[
            (
                "radius",
                json!({ "oneOf": [{ "type": "number" }, { "$ref": "#/$defs/vec2" }] }),
            ),
            (
                "stops",
                json!({
                    "type": "array",
                    "minItems": 1,
                    "items": { "$ref": "#/$defs/gradientStop" }
                }),
            ),
        ],
    )?;
    // Serde aliases of the gradient enums.
    for (enum_name, aliases) in [
        ("PaintKind", &["linear", "radial", "conic"][..]),
        ("GradientUnits", &["userSpaceOnUse"]),
        ("GradientInterpolation", &["linear"]),
    ] {
        defs.get_mut(&def_key(enum_name))
            .and_then(|def| def["enum"].as_array_mut())
            .with_context(|| format!("`{enum_name}` is not an enum def"))?
            .extend(aliases.iter().map(|alias| json!(alias)));
    }
    let stop = object_schema_json::<GradientStopDelegate>(&mut defs, &["offset", "color"], &[])?;
    insert_def(
        &mut defs,
        "GradientStop",
        json!({
            "description": "A color at a point along a gradient: `{ offset, color }` or `[offset, color]`.",
            "oneOf": [
                stop,
                {
                    "type": "array",
                    "prefixItems": [{ "type": "number" }, { "$ref": "#/$defs/color" }],
                    "minItems": 2,
                    "maxItems": 2
                }
            ]
        }),
    );

    insert_def(
        &mut defs,
        "ShaderUniformComponent",
        json!({ "anyOf": [{ "type": "number" }, { "$ref": "#/$defs/expression" }] }),
    );
    insert_def(
        &mut defs,
        "ShaderUniformValue",
        json!({
            "description": "A shader uniform field value: a number, a 2-4 component vector, an expression, or a hex colour for vec3<f32> and vec4<f32> fields.",
            "anyOf": [
                { "type": "number" },
                {
                    "type": "array",
                    "minItems": 2,
                    "maxItems": 4,
                    "items": { "$ref": "#/$defs/shaderUniformComponent" }
                },
                { "$ref": "#/$defs/expression" },
                { "type": "string", "pattern": "^#([0-9a-fA-F]{6}|[0-9a-fA-F]{8})$" }
            ]
        }),
    );
    insert_def(
        &mut defs,
        "ShaderUniforms",
        json!({
            "description": "Shader uniform values by field name, checked against the shader's uniform struct when the node compiles.",
            "type": "object",
            "additionalProperties": { "$ref": "#/$defs/shaderUniformValue" }
        }),
    );
    insert_def(
        &mut defs,
        "ClipPropValue",
        json!({
            "description": "A value for a template prop: a literal of the prop's type, or an expression in the clip's time.",
            "anyOf": [
                { "type": "number" },
                { "type": "string" },
                { "$ref": "#/$defs/vec2" },
                { "$ref": "#/$defs/color" }
            ]
        }),
    );
    insert_def(
        &mut defs,
        "ClipProps",
        json!({
            "description": "Prop values a `clip` passes to its template, by prop name.",
            "type": "object",
            "additionalProperties": { "$ref": "#/$defs/clipPropValue" }
        }),
    );
    Ok(defs)
}

fn def_key(type_name: &str) -> String {
    let mut chars = type_name.chars();
    chars
        .next()
        .map(|first| first.to_lowercase().chain(chars).collect())
        .unwrap_or_default()
}

fn insert_def(defs: &mut serde_json::Map<String, Value>, type_name: &str, mut schema: Value) {
    schema["x-lumen-type"] = json!(type_name);
    defs.insert(def_key(type_name), schema);
}

fn insert_object_def<P: lumen_engine::node::NodeParams>(
    defs: &mut serde_json::Map<String, Value>,
    type_name: &str,
    required: &[&str],
    value_schemas: &[(&str, Value)],
) -> Result<()> {
    let schema = object_schema_json::<P>(defs, required, value_schemas)?;
    insert_def(defs, type_name, schema);
    Ok(())
}

/// An object schema with a field per param of `P`, described like node params, of which
/// `required` must be present and have no default. Enum fields reference a `$def` of their enum,
/// and `value_schemas` replaces the value schemas of fields the metadata can't describe.
fn object_schema_json<P: lumen_engine::node::NodeParams>(
    defs: &mut serde_json::Map<String, Value>,
    required: &[&str],
    value_schemas: &[(&str, Value)],
) -> Result<Value> {
    let properties = P::property_defs();
    let mut specs = param_specs(&properties, &P::default().default_properties())?;
    let mut fields = serde_json::Map::new();
    for (property, spec) in properties.iter().zip(&mut specs) {
        if required.contains(&property.id) {
            spec.default_value = None;
        }
        let replaced = value_schemas.iter().find(|(id, _)| *id == property.id);
        let value_schema = match (replaced, property.enum_def) {
            (Some((_, schema)), _) => schema.clone(),
            (None, Some(enum_def)) => {
                insert_def(defs, enum_def.name, enum_schema_json(spec));
                json!({ "$ref": format!("#/$defs/{}", def_key(enum_def.name)) })
            }
            (None, None) => value_schema_json(spec),
        };
        let mut field = field_schema_json(spec, value_schema);
        // The param kind describes the field's Rust type, not the replaced shape.
        if let (Some(_), Value::Object(object)) = (replaced, &mut field) {
            object.remove("x-lumen-kind");
        }
        fields.insert(spec.id.clone(), field);
    }
    if let Some(id) = required
        .iter()
        .chain(value_schemas.iter().map(|(id, _)| id))
        .find(|id| !fields.contains_key(**id))
    {
        bail!("`{id}` is not a field");
    }
    let mut schema =
        json!({ "type": "object", "properties": fields, "additionalProperties": false });
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    Ok(schema)
}

fn prop_declaration_json(kind: &str, default: Value) -> Value {
    json!({
        "type": "object",
        "required": ["type"],
        "properties": { "type": { "const": kind }, "default": default },
        "additionalProperties": false
    })
}

fn expressions_json() -> Value {
    use lumen_engine::expr::{BUILTINS, Easing, GLOBALS};
    let builtins = BUILTINS
        .iter()
        .map(|spec| {
            let args = spec
                .args
                .iter()
                .map(|arg| json!({ "name": arg.name, "kind": arg.kind.name() }))
                .collect::<Vec<_>>();
            json!({
                "name": spec.name,
                "minArgs": spec.min_args,
                "maxArgs": spec.max_args,
                "args": args,
                "returns": spec.returns.name(),
                "doc": spec.doc,
            })
        })
        .collect::<Vec<_>>();
    let globals = GLOBALS
        .iter()
        .map(|global| json!({ "name": global.name, "doc": global.doc }))
        .collect::<Vec<_>>();
    let easings = Easing::ALL.map(Easing::name);
    json!({ "builtins": builtins, "easings": easings, "globals": globals })
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
    field_schema_json(property, value_schema_json(property))
}

fn value_schema_json(property: &NodeParamSpec) -> Value {
    match property.kind {
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
        NodeParamKind::ShaderUniforms => json!({ "$ref": "#/$defs/shaderUniforms" }),
        NodeParamKind::ClipProps => json!({ "$ref": "#/$defs/clipProps" }),
        NodeParamKind::TextContent => json!({ "$ref": "#/$defs/textContent" }),
        NodeParamKind::TextLine => json!({ "$ref": "#/$defs/textLine" }),
        NodeParamKind::TextHighlight => json!({ "$ref": "#/$defs/textHighlight" }),
        NodeParamKind::Enum => enum_schema_json(property),
    }
}

fn enum_schema_json(property: &NodeParamSpec) -> Value {
    let values = property
        .enum_options
        .iter()
        .map(|option| option.name.clone())
        .collect::<Vec<_>>();
    json!({ "type": "string", "enum": values })
}

/// The schema of an `Override` field: absent inherits, `null` switches the inherited value off.
fn nullable_schema_json(schema: Value) -> Value {
    json!({ "anyOf": [schema, { "type": "null" }] })
}

/// `value_schema` with the param's expression, `null` and metadata keywords.
fn field_schema_json(property: &NodeParamSpec, value_schema: Value) -> Value {
    let mut schema = if property.accepts_expressions {
        json!({ "anyOf": [value_schema, { "$ref": "#/$defs/expression" }] })
    } else {
        value_schema
    };
    if property.nullable {
        schema = nullable_schema_json(schema);
    }
    if let Value::Object(object) = &mut schema {
        object.insert("title".to_string(), json!(property.name));
        object.insert("description".to_string(), json!(property.description));
        object.insert("x-lumen-kind".to_string(), json!(property.kind));
        if let Some(default_value) = &property.default_value {
            object.insert("default".to_string(), json!(default_value));
        }
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
    let params = param_specs(&schema.properties, &schema.default_properties)?;
    if let Some(param) = params.iter().find(|param| param.default_value.is_none()) {
        bail!(
            "missing default value for param `{}` on node `{}`",
            param.id,
            schema.kind
        );
    }
    let default_params = params
        .iter()
        .filter_map(|property| Some((property.id.clone(), property.default_value.clone()?)))
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

fn param_specs(
    properties: &[lumen_engine::node::PropertyDef],
    defaults: &[(&'static str, lumen_engine::node::PropertyValue)],
) -> Result<Vec<NodeParamSpec>> {
    properties
        .iter()
        .map(|property| {
            let default_value = defaults
                .iter()
                .find(|(name, _)| *name == property.id)
                .map(|(_, value)| literal_from_property_value(value, property.enum_def))
                .transpose()?;
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
                default_value,
                accepts_expressions: property.accepts_expressions,
                nullable: property.nullable,
                enum_options,
                constraints: constraints_from_schema(property.constraints),
            })
        })
        .collect()
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
        lumen_engine::node::PropertyKind::ShaderUniforms => NodeParamKind::ShaderUniforms,
        lumen_engine::node::PropertyKind::ClipProps => NodeParamKind::ClipProps,
        lumen_engine::node::PropertyKind::TextContent => NodeParamKind::TextContent,
        lumen_engine::node::PropertyKind::TextLine => NodeParamKind::TextLine,
        lumen_engine::node::PropertyKind::TextHighlight => NodeParamKind::TextHighlight,
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
        lumen_engine::node::PropertyValue::ShaderUniforms(value) => {
            Ok(NodeLiteralValue::ShaderUniforms(value.to_json_value()))
        }
        lumen_engine::node::PropertyValue::ClipProps(value) => {
            Ok(NodeLiteralValue::ClipProps(value.to_json_value()))
        }
        lumen_engine::node::PropertyValue::TextContent(value) => {
            Ok(NodeLiteralValue::TextContent(value.to_json_value()))
        }
        lumen_engine::node::PropertyValue::TextLine(value) => {
            Ok(NodeLiteralValue::TextLine(Value::Object(value.to_json())))
        }
        lumen_engine::node::PropertyValue::TextHighlight(value) => Ok(
            NodeLiteralValue::TextHighlight(Value::Object(value.to_json())),
        ),
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
    fn text_kind_schemas_match_engine_parsing() {
        use lumen_engine::node::{
            Delegated,
            source::text_content::{
                TextContent, TextContentDelegate, TextHighlightDelegate, TextLineDelegate,
                TextSpanDelegate,
            },
        };

        let artifacts = render_definition_artifacts().unwrap();
        let schema: Value = serde_json::from_str(&artifacts.composition_schema_json).unwrap();
        let validator = |def: &str| {
            jsonschema::validator_for(&json!({
                "$defs": schema["$defs"].clone(),
                "$ref": format!("#/$defs/{def}")
            }))
            .unwrap()
        };
        let content = validator("textContent");

        for (def, value) in [
            (
                "textContent",
                TextContent::default().into_delegate().to_json_value(),
            ),
            (
                "textSpan",
                Value::Object(TextSpanDelegate::default().to_json()),
            ),
            (
                "textLine",
                Value::Object(TextLineDelegate::default().to_json()),
            ),
            (
                "textHighlight",
                Value::Object(TextHighlightDelegate::default().to_json()),
            ),
        ] {
            assert!(validator(def).is_valid(&value), "{def} default: {value}");
        }

        let example = json!([
            { "text": "Hello, " },
            {
                "text": "=prop('name')",
                "font_family": "Inter",
                "font_size": 48.0,
                "font_weight": "=frame * 10",
                "font_style": "italic",
                "letter_spacing": -0.5,
                "color": [255, 0, 0, 255],
                "underline": {
                    "style": "wavy",
                    "paint": "=rgb(0, 0, 255)",
                    "thickness": 2.0,
                    "offset": 3.0,
                    "extent": "=vec2(0, frame / 24)"
                },
                "strikethrough": null,
                "highlight": {
                    "paint": [255, 255, 0, 128],
                    "padding": [4.0, 2.0],
                    "radius": 3.0,
                    "extent": [0.0, 1.0]
                }
            }
        ]);
        let parsed = TextContentDelegate::from_json_value(&example).unwrap();
        assert_eq!(parsed.to_json_value(), example);
        assert!(content.is_valid(&example));

        let enum_expressions = json!([{
            "text": "a",
            "font_style": "=frame % 3",
            "underline": { "style": "=1" }
        }]);
        assert!(content.is_valid(&enum_expressions));
        assert!(TextContentDelegate::from_json_value(&enum_expressions).is_ok());

        for invalid in [
            json!(5),
            json!([{ "text": "a", "font_wieght": 700 }]),
            json!([{ "text": "a", "font_size": null }]),
            json!([{ "font_size": 12 }]),
            json!([{ "text": "a", "font_style": "bold" }]),
            json!([{ "text": "a", "underline": "=1" }]),
            json!([{ "text": "a", "highlight": { "radius": "big" } }]),
        ] {
            assert!(!content.is_valid(&invalid), "{invalid}");
            assert!(
                TextContentDelegate::from_json_value(&invalid).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn paint_shader_uniform_and_clip_props_schemas_match_engine_parsing() {
        let manifest = meta_manifest().unwrap();
        let validator = schema_validator();
        let check = |label: &str, composition: Value, valid: bool| {
            assert_eq!(validator.is_valid(&composition), valid, "{label} schema");
            assert_eq!(
                lumen_engine::json::parse_value(&composition).is_ok(),
                valid,
                "{label} engine"
            );
        };

        let stops = json!([[0.0, [255, 0, 0]], { "offset": 1.0, "color": "#0000ff" }]);
        let gradient = |field: &str, value: Value| {
            let mut paint = json!({ "type": "linear", "stops": stops });
            paint[field] = value;
            paint
        };
        let mut paints = vec![
            (json!([255, 0, 0]), true),
            (json!([255, 0, 0, 128]), true),
            (json!("#ff0000"), true),
            (json!("#ff000080"), true),
            (json!("=rgb(255, 0, 0)"), true),
            (
                json!({
                    "type": "radial_gradient",
                    "units": "user_space",
                    "spread": "reflect",
                    "interpolation": "linear_srgb",
                    "start": [0, 0],
                    "end": [1, 0],
                    "center": [0.5, 0.5],
                    "radius": [0.25, 0.5],
                    "angle": 90,
                    "stops": [{ "offset": -0.5, "color": [0, 0, 0, 255] }, [1.5, "#ffffff"]]
                }),
                true,
            ),
            (gradient("radius", json!(0.25)), true),
            (json!("red"), false),
            (json!("#ff00"), false),
            (json!([255, 0]), false),
            (json!([256, 0, 0]), false),
            (json!({ "stops": stops }), false),
            (json!({ "type": "linear" }), false),
            (gradient("stops", json!([])), false),
            (gradient("type", json!("diamond")), false),
            (gradient("units", json!("user")), false),
            (gradient("spread", json!("mirror")), false),
            (gradient("interpolation", json!("oklab")), false),
            (gradient("radius", json!("big")), false),
            (gradient("radius", json!([1])), false),
            (gradient("start", json!("=vec2(0, 0)")), false),
            (gradient("angle", json!("=frame")), false),
            (gradient("stops", json!([[0]])), false),
            (gradient("stops", json!([[0, [0, 0, 0], 1]])), false),
            (gradient("stops", json!([["0", [0, 0, 0]]])), false),
            (gradient("stops", json!([{ "color": [0, 0, 0] }])), false),
            (
                gradient("stops", json!([{ "offset": 0, "color": "=frame" }])),
                false,
            ),
            (
                gradient(
                    "stops",
                    json!([{ "offset": 0, "color": [0, 0, 0], "x": 1 }]),
                ),
                false,
            ),
        ];
        for (field, values) in [
            (
                "type",
                &[
                    "linear",
                    "linear_gradient",
                    "radial",
                    "radial_gradient",
                    "conic",
                    "conic_gradient",
                ][..],
            ),
            (
                "units",
                &["object_bounding_box", "user_space", "userSpaceOnUse"],
            ),
            ("spread", &["pad", "repeat", "reflect"]),
            ("interpolation", &["srgb", "linear_srgb", "linear"]),
        ] {
            paints.extend(
                values
                    .iter()
                    .map(|value| (gradient(field, json!(value)), true)),
            );
        }
        for (paint, valid) in paints {
            let composition = composition_with_node(
                &manifest.node_specs["shape"],
                json!({ "fill_paint": paint }),
            );
            check(&paint.to_string(), composition, valid);
        }

        for (uniforms, valid) in [
            (
                json!({
                    "a": 1,
                    "b": [1, "=frame"],
                    "c": [1, 2, 3],
                    "d": [1, 2, 3, "=frame"],
                    "e": "=vec2(1, 2)",
                    "f": "#ff0000",
                    "g": "#ff000080"
                }),
                true,
            ),
            (json!({ "a": true }), false),
            (json!({ "a": [1] }), false),
            (json!({ "a": [1, 2, 3, 4, 5] }), false),
            (json!({ "a": [1, "x"] }), false),
            (json!({ "a": [1, [2]] }), false),
            (json!({ "a": "red" }), false),
            (json!({ "a": "#ff00" }), false),
            (json!([1, 2]), false),
        ] {
            let composition = composition_with_node(
                &manifest.node_specs["wgsl_shader"],
                json!({ "uniforms": uniforms }),
            );
            check(&uniforms.to_string(), composition, valid);
        }

        let demo: Value =
            serde_json::from_str(include_str!("../../local/demo/clips.json")).unwrap();
        let clip = demo["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .position(|node| node["type"] == "clip")
            .unwrap();
        for (props, valid) in [
            (
                json!({ "title": "=frame", "subtitle": "==x", "accent": [255, 0, 0], "index": 2 }),
                true,
            ),
            (json!({ "accent": "#ff0000", "index": "=frame" }), true),
            (json!({ "index": true }), false),
            (json!({ "index": {} }), false),
            (json!({ "title": null }), false),
            (json!([1]), false),
        ] {
            let mut composition = demo.clone();
            composition["nodes"][clip]["params"]["props"] = props.clone();
            check(&props.to_string(), composition, valid);
        }
    }

    #[test]
    fn compositions_schema_matches_engine_parsing() {
        let validator = schema_validator();
        let demo: Value =
            serde_json::from_str(include_str!("../../local/demo/clips.json")).unwrap();
        assert!(validator.is_valid(&demo), "clips demo should validate");
        lumen_engine::json::parse_value(&demo).unwrap();

        let with_template = |template: Value| {
            let mut value = demo.clone();
            value["compositions"]["title_card"] = template;
            value
        };
        let template = &demo["compositions"]["title_card"];
        for (patch, valid) in [
            (json!({}), true),
            (
                json!({ "props": { "n": { "type": "number", "default": "x" } } }),
                false,
            ),
            (json!({ "props": { "n": { "type": "matrix" } } }), false),
            (json!({ "functions": {} }), false),
        ] {
            let mut template = template.clone();
            for (key, value) in patch.as_object().unwrap() {
                template[key] = value.clone();
            }
            let composition = with_template(template);
            assert_eq!(validator.is_valid(&composition), valid, "{patch}");
            assert_eq!(
                lumen_engine::json::parse_value(&composition).is_ok(),
                valid,
                "{patch}"
            );
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
        assert!(validator.is_valid(&node("text", json!({ "color": "=rgb(255, 0, 0)" }))));
        assert!(validator.is_valid(&node("shape", json!({ "fill_paint": "=#ff0000" }))));
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
        for (signature, valid) in [
            ("rise(start)", true),
            ("pop( )", true),
            ("rise(start", false),
        ] {
            let mut value = node("shape", json!({ "position": "=vec2(0, 0)" }));
            value["functions"] = json!({ signature: "vec2(0, 0)" });
            assert_eq!(validator.is_valid(&value), valid, "{signature}");
            assert_eq!(
                lumen_engine::json::parse_value(&value).is_ok(),
                valid,
                "{signature}"
            );
        }
        assert!(!validator.is_valid(&composition(
            json!({ "id": 1, "type": "background" }),
            vec![json!({ "from_node": 1, "to_node": OUTPUT_ID, "to_port": "source" })],
            json!("=frame"),
        )));
    }
}
