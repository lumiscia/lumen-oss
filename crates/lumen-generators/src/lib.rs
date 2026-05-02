use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use lumen::{
    error::{LumenError, PropertyError},
    node::{
        InputPortDef, NodeDef, NodeId, NodeProperty, OutputPortDef, PortKind, PropertyDef,
        PropertyEval, PropertyKind,
        compositing::{
            boolean::Boolean, merge::Merge, raster_multimerge::RasterMultiMerge, switch::Switch,
        },
        media_output::MediaOutput,
        processing::{
            alpha_premultiply::AlphaPremultiply, blur::Blur, channel_shuffle::ChannelShuffle,
            color_grade::ColorGrade, crop::Crop, curves::Curves, exposure::Exposure,
            hue_saturation::HueSaturation, levels::Levels, matte_cleanup::MatteCleanup, memo::Memo,
            resize::Resize, shadow::Shadow, skia_shader::SkiaShader, time_remap::TimeRemap,
            transform::Transform,
        },
        source::{media_in::MediaIn, solid_color::SolidColor, text::Text},
        vector::{
            path::BezierPath, shape::Shape, shape_renderer::ShapeRenderer,
            vector_merge::VectorMerge, vector_multimerge::VectorMultiMerge,
            vector_stroke_style::VectorStrokeStyle, vector_text::VectorText,
            vector_transform::VectorTransform,
        },
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const DEFAULT_CONFIG_PATH: &str = "crates/lumen-generators/package.config.json";
pub const DEFAULT_OUTPUT_DIR: &str = "packages/lumen-node-specs";
pub const DEFAULT_DEFINITIONS_OUTPUT_DIR: &str = "generated/lumen-definitions";
pub const META_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliArgs {
    pub target: GenerateTarget,
    pub config: PathBuf,
    pub out_dir: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerateTarget {
    Definitions,
    MetaPackage,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageConfig {
    pub name: String,
    pub version: String,
    pub description: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub bugs: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub publish_access: Option<String>,
    #[serde(default)]
    pub private: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeCategory {
    Compositing,
    Processing,
    Source,
    Vector,
    Output,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodePortKind {
    RasterFrame,
    Surface,
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
    Vec2([f64; 2]),
    FloatVec(Vec<f64>),
    IntVec(Vec<i64>),
    StringVec(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum NodeEnumOptionValue {
    Int(i64),
    String(String),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeEnumOptionSpec {
    pub label: String,
    pub value: NodeEnumOptionValue,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodePropertyMetadata {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enum_options: Vec<NodeEnumOptionSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeInputPortSpec {
    pub name: String,
    pub kind: NodePortKind,
    pub optional: bool,
    pub variadic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<NodePropertyMetadata>,
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

#[derive(Debug, Clone, Copy)]
struct NodeMetadata {
    kind: &'static str,
    label: &'static str,
    description: &'static str,
    category: NodeCategory,
}

pub fn parse_args_from<I>(args: I) -> Result<CliArgs>
where
    I: IntoIterator<Item = String>,
{
    let mut config = PathBuf::from(DEFAULT_CONFIG_PATH);
    let mut out_dir = PathBuf::from(DEFAULT_OUTPUT_DIR);
    let mut target = GenerateTarget::MetaPackage;

    let mut args = args.into_iter().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "definitions" => {
                target = GenerateTarget::Definitions;
                if out_dir == PathBuf::from(DEFAULT_OUTPUT_DIR) {
                    out_dir = PathBuf::from(DEFAULT_DEFINITIONS_OUTPUT_DIR);
                }
            }
            "meta-package" => target = GenerateTarget::MetaPackage,
            "--config" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow!("missing value for --config"))?;
                config = PathBuf::from(value);
            }
            "--out-dir" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow!("missing value for --out-dir"))?;
                out_dir = PathBuf::from(value);
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            unknown => bail!("unknown argument: {unknown}"),
        }
    }

    Ok(CliArgs {
        target,
        config,
        out_dir,
    })
}

pub fn print_usage() {
    println!(
        "Usage: cargo run -p lumen-generators -- [definitions|meta-package] [--config <path>] [--out-dir <path>]"
    );
}

pub fn read_package_config(path: &Path) -> Result<PackageConfig> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read package config `{}`", path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse package config `{}`", path.display()))
}

pub fn generate(args: &CliArgs) -> Result<()> {
    match args.target {
        GenerateTarget::Definitions => generate_definitions(&args.out_dir),
        GenerateTarget::MetaPackage => generate_meta_package(&args.config, &args.out_dir),
    }
}

pub fn generate_definitions(out_dir: &Path) -> Result<()> {
    let artifacts = render_definition_artifacts()?;
    validate_generated_artifacts(
        &artifacts.meta_json,
        &artifacts.meta_schema_json,
        &artifacts.composition_schema_json,
    )?;

    fs::create_dir_all(out_dir)
        .with_context(|| format!("failed to create output directory `{}`", out_dir.display()))?;

    write_if_changed(&out_dir.join("definitions/meta.json"), &artifacts.meta_json)?;
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

pub fn generate_meta_package(config_path: &Path, out_dir: &Path) -> Result<()> {
    let config = read_package_config(config_path)?;
    let artifacts = render_definition_artifacts()?;
    let package_json = render_package_json(&config).context("failed to render package.json")?;
    let index_js = render_index_js().context("failed to render index.js")?;
    let index_dts = render_index_dts(&artifacts.manifest).context("failed to render index.d.ts")?;

    validate_generated_artifacts(
        &artifacts.meta_json,
        &artifacts.meta_schema_json,
        &artifacts.composition_schema_json,
    )?;

    fs::create_dir_all(out_dir)
        .with_context(|| format!("failed to create output directory `{}`", out_dir.display()))?;

    write_if_changed(&out_dir.join("package.json"), &package_json)?;
    write_if_changed(&out_dir.join("meta.json"), &artifacts.meta_json)?;
    write_if_changed(&out_dir.join("index.js"), &index_js)?;
    write_if_changed(&out_dir.join("index.d.ts"), &index_dts)?;
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
    let manifest = meta_manifest().context("failed to build node schema manifest")?;
    let meta_json = render_meta_json(&manifest).context("failed to render meta.json")?;
    let meta_schema_json =
        render_meta_schema_json().context("failed to render meta JSON schema")?;
    let composition_schema_json = render_composition_schema_json(&manifest)
        .context("failed to render composition JSON schema")?;

    Ok(DefinitionArtifacts {
        manifest,
        meta_json,
        meta_schema_json,
        composition_schema_json,
    })
}

pub fn validate_generated_artifacts(
    meta_json: &str,
    meta_schema_json: &str,
    composition_schema_json: &str,
) -> Result<()> {
    let node_specs = parse_generated_json("meta.json", meta_json)?;
    let meta_schema = parse_generated_json("schemas/meta.schema.json", meta_schema_json)?;
    let composition_schema =
        parse_generated_json("schemas/composition.schema.json", composition_schema_json)?;

    validate_json_schema_document("schemas/meta.schema.json", &meta_schema)?;
    validate_json_schema_document("schemas/composition.schema.json", &composition_schema)?;
    validate_json_instance("meta.json", &meta_schema, &node_specs)?;

    Ok(())
}

pub fn meta_manifest() -> Result<MetaManifest> {
    let specs = vec![
        build_node_spec::<Boolean>(NodeMetadata {
            kind: "boolean",
            label: "Boolean",
            description: "Combines raster inputs using a boolean mask operation.",
            category: NodeCategory::Compositing,
        })?,
        build_node_spec::<Merge>(NodeMetadata {
            kind: "merge",
            label: "Merge",
            description: "Composites an overlay on top of a base raster input.",
            category: NodeCategory::Compositing,
        })?,
        build_node_spec::<RasterMultiMerge>(NodeMetadata {
            kind: "raster_multimerge",
            label: "Raster Multi Merge",
            description: "Composites multiple raster inputs in sequence.",
            category: NodeCategory::Compositing,
        })?,
        build_node_spec::<Switch>(NodeMetadata {
            kind: "switch",
            label: "Switch",
            description: "Selects one raster input according to the configured frame ranges.",
            category: NodeCategory::Compositing,
        })?,
        build_node_spec::<AlphaPremultiply>(NodeMetadata {
            kind: "alpha_premultiply",
            label: "Alpha Premultiply",
            description: "Premultiplies or unpremultiplies raster color channels by alpha.",
            category: NodeCategory::Processing,
        })?,
        build_node_spec::<Blur>(NodeMetadata {
            kind: "blur",
            label: "Blur",
            description: "Applies a gaussian blur to a raster input.",
            category: NodeCategory::Processing,
        })?,
        build_node_spec::<ChannelShuffle>(NodeMetadata {
            kind: "channel_shuffle",
            label: "Channel Shuffle",
            description: "Maps output RGBA channels from source channels or constants.",
            category: NodeCategory::Processing,
        })?,
        build_node_spec::<ColorGrade>(NodeMetadata {
            kind: "color_grade",
            label: "Color Grade",
            description: "Applies an inline LUT to a raster input.",
            category: NodeCategory::Processing,
        })?,
        build_node_spec::<Crop>(NodeMetadata {
            kind: "crop",
            label: "Crop",
            description: "Crops a raster input to the configured bounds.",
            category: NodeCategory::Processing,
        })?,
        build_node_spec::<Curves>(NodeMetadata {
            kind: "curves",
            label: "Curves",
            description: "Remaps raster color channels through editable curves.",
            category: NodeCategory::Processing,
        })?,
        build_node_spec::<Exposure>(NodeMetadata {
            kind: "exposure",
            label: "Exposure",
            description: "Adjusts exposure, contrast, and offset for a raster input.",
            category: NodeCategory::Processing,
        })?,
        build_node_spec::<HueSaturation>(NodeMetadata {
            kind: "hue_saturation",
            label: "Hue/Saturation",
            description: "Adjusts hue, saturation, and lightness for a raster input.",
            category: NodeCategory::Processing,
        })?,
        build_node_spec::<Levels>(NodeMetadata {
            kind: "levels",
            label: "Levels",
            description: "Remaps raster black, white, gamma, and output levels.",
            category: NodeCategory::Processing,
        })?,
        build_node_spec::<MatteCleanup>(NodeMetadata {
            kind: "matte_cleanup",
            label: "Matte Cleanup",
            description: "Thresholds, grows, or shrinks raster alpha mattes.",
            category: NodeCategory::Processing,
        })?,
        build_node_spec::<Memo>(NodeMetadata {
            kind: "memo",
            label: "Memo",
            description: "Caches a raster input for reuse across evaluations.",
            category: NodeCategory::Processing,
        })?,
        build_node_spec::<Resize>(NodeMetadata {
            kind: "resize",
            label: "Resize",
            description: "Resizes a raster input using the configured mode and sampling.",
            category: NodeCategory::Processing,
        })?,
        build_node_spec::<Shadow>(NodeMetadata {
            kind: "shadow",
            label: "Shadow",
            description: "Applies a drop shadow to a raster input.",
            category: NodeCategory::Processing,
        })?,
        build_node_spec::<SkiaShader>(NodeMetadata {
            kind: "skia_shader",
            label: "Skia Shader",
            description: "Runs a Skia runtime shader over a raster input.",
            category: NodeCategory::Processing,
        })?,
        build_node_spec::<TimeRemap>(NodeMetadata {
            kind: "time_remap",
            label: "Time Remap",
            description: "Evaluates a raster input at a remapped source frame.",
            category: NodeCategory::Processing,
        })?,
        build_node_spec::<Transform>(NodeMetadata {
            kind: "transform",
            label: "Transform",
            description: "Moves, scales, or rotates a raster input.",
            category: NodeCategory::Processing,
        })?,
        build_node_spec::<MediaIn>(NodeMetadata {
            kind: "media_in",
            label: "Media In",
            description: "Loads an image or video source into the graph.",
            category: NodeCategory::Source,
        })?,
        build_node_spec::<SolidColor>(NodeMetadata {
            kind: "solid_color",
            label: "Solid Color",
            description: "Creates a raster layer filled with a single color.",
            category: NodeCategory::Source,
        })?,
        build_node_spec::<Text>(NodeMetadata {
            kind: "text",
            label: "Text",
            description: "Renders text directly to a raster layer.",
            category: NodeCategory::Source,
        })?,
        build_node_spec::<BezierPath>(NodeMetadata {
            kind: "bezier_path",
            label: "Bezier Path",
            description: "Creates vector path geometry from SVG path commands.",
            category: NodeCategory::Vector,
        })?,
        build_node_spec::<Shape>(NodeMetadata {
            kind: "shape",
            label: "Shape",
            description: "Creates vector geometry that can be rasterized later.",
            category: NodeCategory::Vector,
        })?,
        build_node_spec::<ShapeRenderer>(NodeMetadata {
            kind: "shape_renderer",
            label: "Shape Renderer",
            description: "Rasterizes an incoming vector layer.",
            category: NodeCategory::Vector,
        })?,
        build_node_spec::<VectorMerge>(NodeMetadata {
            kind: "vector_merge",
            label: "Vector Merge",
            description: "Combines two vector inputs into one vector output.",
            category: NodeCategory::Vector,
        })?,
        build_node_spec::<VectorMultiMerge>(NodeMetadata {
            kind: "vector_multimerge",
            label: "Vector Multi Merge",
            description: "Combines multiple vector inputs into one vector output.",
            category: NodeCategory::Vector,
        })?,
        build_node_spec::<VectorStrokeStyle>(NodeMetadata {
            kind: "vector_stroke_style",
            label: "Vector Stroke Style",
            description: "Applies fill and stroke defaults to vector inputs.",
            category: NodeCategory::Vector,
        })?,
        build_node_spec::<VectorText>(NodeMetadata {
            kind: "vector_text",
            label: "Vector Text",
            description: "Creates vector text that can be composed before rasterization.",
            category: NodeCategory::Vector,
        })?,
        build_node_spec::<VectorTransform>(NodeMetadata {
            kind: "vector_transform",
            label: "Vector Transform",
            description: "Transforms vector inputs before rasterization.",
            category: NodeCategory::Vector,
        })?,
        build_node_spec::<MediaOutput>(NodeMetadata {
            kind: "media_output",
            label: "Media Output",
            description: "Final raster sink used by the renderer.",
            category: NodeCategory::Output,
        })?,
    ];

    let node_kinds = specs.iter().map(|spec| spec.kind.clone()).collect();
    let node_specs = specs
        .into_iter()
        .map(|spec| (spec.kind.clone(), spec))
        .collect::<BTreeMap<_, _>>();

    Ok(MetaManifest {
        schema_version: META_SCHEMA_VERSION,
        node_kinds,
        node_specs,
    })
}

pub fn render_package_json(config: &PackageConfig) -> Result<String> {
    if config.name.trim().is_empty() {
        bail!("package config name cannot be empty");
    }
    if config.version.trim().is_empty() {
        bail!("package config version cannot be empty");
    }
    if config.description.trim().is_empty() {
        bail!("package config description cannot be empty");
    }

    let exports = json!({
        ".": {
            "types": "./index.d.ts",
            "import": "./index.js",
            "default": "./index.js"
        },
        "./meta": {
            "default": "./meta.json"
        },
        "./schemas/meta": {
            "default": "./schemas/meta.schema.json"
        },
        "./schemas/composition": {
            "default": "./schemas/composition.schema.json"
        },
        "./package.json": "./package.json"
    });

    let mut package = Map::new();
    package.insert("name".to_string(), Value::String(config.name.clone()));
    package.insert("version".to_string(), Value::String(config.version.clone()));
    package.insert(
        "description".to_string(),
        Value::String(config.description.clone()),
    );
    package.insert("type".to_string(), Value::String("module".to_string()));
    package.insert("main".to_string(), Value::String("./index.js".to_string()));
    package.insert(
        "types".to_string(),
        Value::String("./index.d.ts".to_string()),
    );
    package.insert("sideEffects".to_string(), Value::Bool(false));
    package.insert(
        "files".to_string(),
        json!([
            "index.js",
            "index.d.ts",
            "meta.json",
            "schemas",
            "package.json"
        ]),
    );
    package.insert("exports".to_string(), exports);

    insert_optional_string(&mut package, "author", &config.author);
    insert_optional_string(&mut package, "license", &config.license);
    insert_optional_string(&mut package, "homepage", &config.homepage);
    insert_optional_string(&mut package, "repository", &config.repository);
    insert_optional_string(&mut package, "bugs", &config.bugs);
    if !config.keywords.is_empty() {
        package.insert("keywords".to_string(), json!(config.keywords));
    }
    if let Some(access) = &config.publish_access {
        package.insert("publishConfig".to_string(), json!({ "access": access }));
    }
    if config.private {
        package.insert("private".to_string(), Value::Bool(true));
    }

    Ok(format!(
        "{}\n",
        serde_json::to_string_pretty(&Value::Object(package))?
    ))
}

pub fn render_meta_json(manifest: &MetaManifest) -> Result<String> {
    Ok(format!("{}\n", serde_json::to_string_pretty(manifest)?))
}

pub fn render_index_js() -> Result<String> {
    Ok("import NODE_SCHEMA from \"./meta.json\" with { type: \"json\" };\n\nexport { NODE_SCHEMA };\nexport const SCHEMA_VERSION = NODE_SCHEMA.schemaVersion;\nexport const NODE_KINDS = NODE_SCHEMA.nodeKinds;\nexport const NODE_SPECS = NODE_SCHEMA.nodeSpecs;\n".to_string())
}

pub fn render_index_dts(manifest: &MetaManifest) -> Result<String> {
    let node_kind_union = manifest
        .node_kinds
        .iter()
        .map(|kind| format!("{kind:?}"))
        .collect::<Vec<_>>()
        .join(" | ");
    let node_spec_members = manifest
        .node_kinds
        .iter()
        .map(|kind| format!("  readonly {kind:?}: NodeSpec;"))
        .collect::<Vec<_>>()
        .join("\n");

    Ok(format!(
        "export type NodeCategory = \"compositing\" | \"processing\" | \"source\" | \"vector\" | \"output\";\nexport type NodePortKind = \"raster_frame\" | \"surface\" | \"vector\";\nexport type NodePropertyKind = \"float\" | \"int\" | \"bool\" | \"string\" | \"color\" | \"vec2\";\nexport type NodeLiteralValue = boolean | number | string | readonly [number, number] | readonly [number, number, number, number] | readonly number[] | readonly string[];\nexport type NodeEnumOptionValue = number | string;\nexport type NodeKind = {node_kind_union};\n\nexport interface NodeEnumOptionSpec {{\n  readonly label: string;\n  readonly value: NodeEnumOptionValue;\n}}\n\nexport interface NodePropertyMetadata {{\n  readonly enumOptions?: readonly NodeEnumOptionSpec[];\n}}\n\nexport interface NodeInputPortSpec {{\n  readonly name: string;\n  readonly kind: NodePortKind;\n  readonly optional: boolean;\n  readonly variadic: boolean;\n}}\n\nexport interface NodeOutputPortSpec {{\n  readonly name: string;\n  readonly kind: NodePortKind;\n}}\n\nexport interface NodePropertySpec {{\n  readonly name: string;\n  readonly kind: NodePropertyKind;\n  readonly defaultValue: NodeLiteralValue;\n  readonly metadata?: NodePropertyMetadata;\n}}\n\nexport interface NodeSpec {{\n  readonly kind: NodeKind;\n  readonly label: string;\n  readonly description: string;\n  readonly category: NodeCategory;\n  readonly inputs: readonly NodeInputPortSpec[];\n  readonly outputs: readonly NodeOutputPortSpec[];\n  readonly properties: readonly NodePropertySpec[];\n  readonly defaultProperties: Readonly<Record<string, NodeLiteralValue>>;\n}}\n\nexport interface MetaManifest {{\n  readonly schemaVersion: {META_SCHEMA_VERSION};\n  readonly nodeKinds: readonly NodeKind[];\n  readonly nodeSpecs: Readonly<Record<NodeKind, NodeSpec>>;\n}}\n\nexport declare const NODE_SCHEMA: {{\n  readonly schemaVersion: {META_SCHEMA_VERSION};\n  readonly nodeKinds: readonly NodeKind[];\n  readonly nodeSpecs: {{\n{node_spec_members}\n  }};\n}};\n\nexport declare const SCHEMA_VERSION: typeof NODE_SCHEMA.schemaVersion;\nexport declare const NODE_KINDS: typeof NODE_SCHEMA.nodeKinds;\nexport declare const NODE_SPECS: typeof NODE_SCHEMA.nodeSpecs;\n"
    ))
}

pub fn render_meta_schema_json() -> Result<String> {
    let schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://schemas.lumiscia.com/lumen/meta.schema.json",
        "title": "Lumen Metadata",
        "type": "object",
        "required": ["schemaVersion", "nodeKinds", "nodeSpecs"],
        "additionalProperties": false,
        "properties": {
            "schemaVersion": { "type": "integer", "minimum": 1 },
            "nodeKinds": {
                "type": "array",
                "items": { "type": "string" },
                "uniqueItems": true
            },
            "nodeSpecs": {
                "type": "object",
                "additionalProperties": { "$ref": "#/$defs/nodeSpec" }
            }
        },
        "$defs": {
            "nodeSpec": {
                "type": "object",
                "required": [
                    "kind",
                    "label",
                    "description",
                    "category",
                    "inputs",
                    "outputs",
                    "properties",
                    "defaultProperties"
                ],
                "additionalProperties": false,
                "properties": {
                    "kind": { "type": "string" },
                    "label": { "type": "string" },
                    "description": { "type": "string" },
                    "category": {
                        "enum": ["compositing", "processing", "source", "vector", "output"]
                    },
                    "inputs": {
                        "type": "array",
                        "items": { "$ref": "#/$defs/inputPort" }
                    },
                    "outputs": {
                        "type": "array",
                        "items": { "$ref": "#/$defs/outputPort" }
                    },
                    "properties": {
                        "type": "array",
                        "items": { "$ref": "#/$defs/property" }
                    },
                    "defaultProperties": {
                        "type": "object"
                    }
                }
            },
            "inputPort": {
                "type": "object",
                "required": ["name", "kind", "optional", "variadic"],
                "additionalProperties": false,
                "properties": {
                    "name": { "type": "string" },
                    "kind": { "$ref": "#/$defs/portKind" },
                    "optional": { "type": "boolean" },
                    "variadic": { "type": "boolean" }
                }
            },
            "outputPort": {
                "type": "object",
                "required": ["name", "kind"],
                "additionalProperties": false,
                "properties": {
                    "name": { "type": "string" },
                    "kind": { "$ref": "#/$defs/portKind" }
                }
            },
            "property": {
                "type": "object",
                "required": ["name", "kind", "defaultValue"],
                "additionalProperties": false,
                "properties": {
                    "name": { "type": "string" },
                    "kind": { "enum": ["float", "int", "bool", "string", "color", "vec2"] },
                    "defaultValue": true,
                    "metadata": { "type": "object" }
                }
            },
            "portKind": { "enum": ["raster_frame", "surface", "vector"] }
        }
    });

    Ok(format!("{}\n", serde_json::to_string_pretty(&schema)?))
}

pub fn render_composition_schema_json(manifest: &MetaManifest) -> Result<String> {
    let node_refs = manifest
        .node_kinds
        .iter()
        .map(|kind| json!({ "$ref": format!("#/$defs/nodes/{kind}") }))
        .collect::<Vec<_>>();
    let node_defs = manifest
        .node_specs
        .iter()
        .map(|(kind, spec)| (kind.clone(), node_schema(spec)))
        .collect::<Map<_, _>>();

    let schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://schemas.lumiscia.com/lumen/composition.schema.json",
        "title": "Lumen Composition",
        "type": "object",
        "required": ["timeline", "render_settings", "nodes", "connections"],
        "additionalProperties": true,
        "properties": {
            "$schema": { "type": "string" },
            "lumenSchemaVersion": { "type": "string" },
            "schemaVersion": { "type": "string" },
            "metadata": {
                "type": "object",
                "additionalProperties": true,
                "properties": {
                    "name": { "type": "string" }
                }
            },
            "timeline": {
                "type": "object",
                "required": ["fps", "duration_frames"],
                "additionalProperties": false,
                "properties": {
                    "fps": { "type": "number", "exclusiveMinimum": 0 },
                    "duration_frames": { "type": "integer", "minimum": 1 }
                }
            },
            "render_settings": {
                "type": "object",
                "required": ["width", "height"],
                "additionalProperties": false,
                "properties": {
                    "width": { "type": "integer", "minimum": 1 },
                    "height": { "type": "integer", "minimum": 1 },
                    "background_color": { "$ref": "#/$defs/color" }
                }
            },
            "nodes": {
                "type": "array",
                "items": { "oneOf": node_refs }
            },
            "connections": {
                "type": "array",
                "items": { "$ref": "#/$defs/connection" }
            }
        },
        "$defs": {
            "nodeId": { "type": "integer", "minimum": 0 },
            "portName": { "type": "string", "minLength": 1 },
            "expression": { "type": "string", "pattern": "^=" },
            "color": {
                "oneOf": [
                    {
                        "type": "array",
                        "prefixItems": [
                            { "type": "integer", "minimum": 0, "maximum": 255 },
                            { "type": "integer", "minimum": 0, "maximum": 255 },
                            { "type": "integer", "minimum": 0, "maximum": 255 }
                        ],
                        "minItems": 3,
                        "maxItems": 3
                    },
                    {
                        "type": "array",
                        "prefixItems": [
                            { "type": "integer", "minimum": 0, "maximum": 255 },
                            { "type": "integer", "minimum": 0, "maximum": 255 },
                            { "type": "integer", "minimum": 0, "maximum": 255 },
                            { "type": "integer", "minimum": 0, "maximum": 255 }
                        ],
                        "minItems": 4,
                        "maxItems": 4
                    },
                    {
                        "type": "string",
                        "pattern": "^#(?:[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$"
                    }
                ]
            },
            "vec2": {
                "type": "array",
                "prefixItems": [{ "type": "number" }, { "type": "number" }],
                "minItems": 2,
                "maxItems": 2
            },
            "connection": {
                "type": "object",
                "required": ["from_node", "to_node", "to_port"],
                "additionalProperties": false,
                "properties": {
                    "from_node": { "$ref": "#/$defs/nodeId" },
                    "from_port": { "$ref": "#/$defs/portName", "default": "output" },
                    "to_node": { "$ref": "#/$defs/nodeId" },
                    "to_port": { "$ref": "#/$defs/portName" }
                }
            },
            "nodes": node_defs
        }
    });

    Ok(format!("{}\n", serde_json::to_string_pretty(&schema)?))
}

fn node_schema(spec: &NodeSpec) -> Value {
    let property_schemas = spec
        .properties
        .iter()
        .map(|property| {
            (
                property.name.clone(),
                property_value_schema(property.kind, true),
            )
        })
        .collect::<Map<_, _>>();

    json!({
        "type": "object",
        "required": ["id", "type"],
        "additionalProperties": false,
        "properties": {
            "id": { "$ref": "#/$defs/nodeId" },
            "type": { "const": spec.kind },
            "properties": {
                "type": "object",
                "additionalProperties": false,
                "properties": property_schemas
            },
            "map": {
                "type": "object",
                "additionalProperties": {
                    "type": "array",
                    "prefixItems": [
                        { "type": "integer", "minimum": 0 },
                        { "type": "integer", "minimum": 0 }
                    ],
                    "minItems": 2,
                    "maxItems": 2
                }
            }
        }
    })
}

fn property_value_schema(kind: NodePropertyKind, expressions: bool) -> Value {
    let literal = match kind {
        NodePropertyKind::Float => json!({ "type": "number" }),
        NodePropertyKind::Int => json!({ "type": "integer" }),
        NodePropertyKind::Bool => json!({ "type": "boolean" }),
        NodePropertyKind::String => json!({ "type": "string" }),
        NodePropertyKind::Color => json!({ "$ref": "#/$defs/color" }),
        NodePropertyKind::Vec2 => json!({ "$ref": "#/$defs/vec2" }),
    };

    if expressions {
        json!({ "oneOf": [literal, { "$ref": "#/$defs/expression" }] })
    } else {
        literal
    }
}

fn build_node_spec<T>(metadata: NodeMetadata) -> Result<NodeSpec>
where
    T: Default + NodeDef + PropertyEval,
{
    let node = T::default();
    let mut default_properties = BTreeMap::new();
    let mut properties = Vec::new();

    for property_def in <T as NodeDef>::property_defs() {
        let value = node.get_property(property_def.name)?.ok_or_else(|| {
            LumenError::Property(PropertyError::MissingProperty {
                node_id: NodeId::new(0),
                property_path: property_def.name.to_string(),
            })
        })?;
        let default_value = literal_from_property(property_def, value)?;

        default_properties.insert(property_def.name.to_string(), default_value.clone());
        properties.push(NodePropertySpec {
            name: property_def.name.to_string(),
            kind: property_kind_from_def(property_def.expected),
            default_value,
            metadata: None,
        });
    }

    Ok(NodeSpec {
        kind: metadata.kind.to_string(),
        label: metadata.label.to_string(),
        description: metadata.description.to_string(),
        category: metadata.category,
        inputs: T::input_port_defs()
            .iter()
            .map(input_port_spec_from_def)
            .collect(),
        outputs: T::output_port_defs()
            .iter()
            .map(output_port_spec_from_def)
            .collect(),
        properties,
        default_properties,
    })
}

fn input_port_spec_from_def(def: &InputPortDef) -> NodeInputPortSpec {
    NodeInputPortSpec {
        name: def.name.to_string(),
        kind: port_kind_from_def(def.kind),
        optional: def.optional,
        variadic: def.variadic,
    }
}

fn output_port_spec_from_def(def: &OutputPortDef) -> NodeOutputPortSpec {
    NodeOutputPortSpec {
        name: def.name.to_string(),
        kind: port_kind_from_def(def.kind),
    }
}

fn port_kind_from_def(kind: PortKind) -> NodePortKind {
    match kind {
        PortKind::GpuImageFrame => NodePortKind::RasterFrame,
        PortKind::Surface => NodePortKind::Surface,
        PortKind::Vector => NodePortKind::Vector,
    }
}

fn property_kind_from_def(kind: PropertyKind) -> NodePropertyKind {
    match kind {
        PropertyKind::Float => NodePropertyKind::Float,
        PropertyKind::Int => NodePropertyKind::Int,
        PropertyKind::Bool => NodePropertyKind::Bool,
        PropertyKind::String => NodePropertyKind::String,
        PropertyKind::Color => NodePropertyKind::Color,
        PropertyKind::Vec2 => NodePropertyKind::Vec2,
    }
}

fn literal_from_property(def: &PropertyDef, value: NodeProperty) -> Result<NodeLiteralValue> {
    match value {
        NodeProperty::Float(value) => Ok(NodeLiteralValue::Float(value)),
        NodeProperty::Int(value) => Ok(NodeLiteralValue::Int(value)),
        NodeProperty::Bool(value) => Ok(NodeLiteralValue::Bool(value)),
        NodeProperty::String(value) => Ok(NodeLiteralValue::String(value)),
        NodeProperty::Color(value) => Ok(NodeLiteralValue::Color(value)),
        NodeProperty::Vec2((x, y)) => Ok(NodeLiteralValue::Vec2([x, y])),
        NodeProperty::FloatVec(values) => Ok(NodeLiteralValue::FloatVec(values)),
        NodeProperty::IntVec(values) => Ok(NodeLiteralValue::IntVec(values)),
        NodeProperty::StringVec(values) => Ok(NodeLiteralValue::StringVec(values)),
        NodeProperty::Expr(_) => Err(LumenError::Property(PropertyError::InvalidType {
            node_id: NodeId::new(0),
            property_path: def.name.to_string(),
            expected: "literal default value",
            actual: "expression",
        })
        .into()),
    }
}

fn insert_optional_string(target: &mut Map<String, Value>, key: &str, value: &Option<String>) {
    if let Some(value) = value.as_ref().filter(|value| !value.trim().is_empty()) {
        target.insert(key.to_string(), Value::String(value.clone()));
    }
}

fn parse_generated_json(name: &str, raw: &str) -> Result<Value> {
    serde_json::from_str(raw).with_context(|| format!("generated `{name}` is not valid JSON"))
}

fn validate_json_schema_document(name: &str, schema: &Value) -> Result<()> {
    jsonschema::meta::validate(schema)
        .map_err(|error| anyhow!("generated `{name}` is not a valid JSON Schema: {error}"))
}

fn validate_json_instance(name: &str, schema: &Value, instance: &Value) -> Result<()> {
    let validator = jsonschema::validator_for(schema)
        .with_context(|| format!("failed to compile JSON Schema for `{name}`"))?;
    validator
        .validate(instance)
        .map_err(|error| anyhow!("generated `{name}` failed JSON Schema validation: {error}"))
}

fn write_if_changed(path: &Path, content: &str) -> Result<()> {
    let should_write = match fs::read_to_string(path) {
        Ok(existing) => existing != content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read existing file `{}`", path.display()));
        }
    };

    if should_write {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create directory `{}`", parent.display()))?;
        }
        fs::write(path, content)
            .with_context(|| format!("failed to write `{}`", path.display()))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use tempfile::tempdir;

    use super::{
        CliArgs, GenerateTarget, PackageConfig, generate_meta_package, meta_manifest,
        parse_args_from, render_composition_schema_json, render_index_dts, render_index_js,
        render_meta_json, render_meta_schema_json, render_package_json,
        validate_generated_artifacts,
    };

    #[test]
    fn parse_args_uses_defaults() {
        let args = parse_args_from(["lumen-generators".to_string()]).expect("args should parse");
        assert_eq!(
            args,
            CliArgs {
                target: GenerateTarget::MetaPackage,
                config: PathBuf::from("crates/lumen-generators/package.config.json"),
                out_dir: PathBuf::from("packages/lumen-node-specs"),
            }
        );
    }

    #[test]
    fn manifest_includes_expected_node_specs() {
        let manifest = meta_manifest().expect("node schema manifest should build");
        let solid_color = manifest
            .node_specs
            .get("solid_color")
            .expect("solid_color spec should exist");

        assert_eq!(manifest.schema_version, 1);
        assert!(
            manifest
                .node_kinds
                .iter()
                .any(|kind| kind == "media_output")
        );
        assert_eq!(solid_color.label, "Solid Color");
    }

    #[test]
    fn renders_publishable_package_manifest() {
        let package_json = render_package_json(&PackageConfig {
            name: "lumen-node-specs".to_string(),
            version: "1.2.3".to_string(),
            description: "Generated Lumen node metadata.".to_string(),
            author: None,
            license: Some("MIT".to_string()),
            homepage: None,
            repository: Some("https://example.com/repo.git".to_string()),
            bugs: None,
            keywords: vec!["lumen".to_string()],
            publish_access: Some("public".to_string()),
            private: false,
        })
        .expect("package manifest should render");

        assert!(package_json.contains("\"name\": \"lumen-node-specs\""));
        assert!(package_json.contains("\"publishConfig\""));
        assert!(package_json.contains("\"exports\""));
        assert!(package_json.contains("meta.json"));
    }

    #[test]
    fn renders_typescript_javascript_and_json_exports() {
        let manifest = meta_manifest().expect("manifest should build");
        let js = render_index_js().expect("js should render");
        let dts = render_index_dts(&manifest).expect("dts should render");
        let json = render_meta_json(&manifest).expect("json should render");

        assert!(js.contains("./meta.json"));
        assert!(js.contains("export const NODE_SPECS"));
        assert!(dts.contains("export type NodeKind ="));
        assert!(dts.contains("\"solid_color\""));
        assert!(json.contains("\"nodeSpecs\""));
    }

    #[test]
    fn renders_json_schemas() {
        let manifest = meta_manifest().expect("manifest should build");
        let meta_schema = render_meta_schema_json().expect("meta schema should render");
        let composition_schema =
            render_composition_schema_json(&manifest).expect("composition schema should render");

        assert!(meta_schema.contains("Lumen Metadata"));
        assert!(composition_schema.contains("Lumen Composition"));
        assert!(composition_schema.contains("\"solid_color\""));
    }

    #[test]
    fn generated_json_artifacts_validate() {
        let manifest = meta_manifest().expect("manifest should build");
        let meta_json = render_meta_json(&manifest).expect("meta should render");
        let meta_schema = render_meta_schema_json().expect("meta schema should render");
        let composition_schema =
            render_composition_schema_json(&manifest).expect("composition schema should render");

        validate_generated_artifacts(&meta_json, &meta_schema, &composition_schema)
            .expect("generated artifacts should validate");
    }

    #[test]
    fn generate_meta_package_writes_expected_files() {
        let dir = tempdir().expect("tempdir should exist");
        let config_path = dir.path().join("config.json");
        let out_dir = dir.path().join("pkg");

        fs::write(
            &config_path,
            r#"{
  "name": "lumen-node-specs",
  "version": "0.1.0",
  "description": "Generated Lumen node metadata."
}"#,
        )
        .expect("config should be written");

        generate_meta_package(&config_path, &out_dir).expect("package generation should succeed");

        assert!(out_dir.join("package.json").exists());
        assert!(out_dir.join("meta.json").exists());
        assert!(out_dir.join("index.js").exists());
        assert!(out_dir.join("index.d.ts").exists());
        assert!(out_dir.join("schemas/meta.schema.json").exists());
        assert!(out_dir.join("schemas/composition.schema.json").exists());
    }
}
