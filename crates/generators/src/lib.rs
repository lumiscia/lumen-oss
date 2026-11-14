// The composition schema is one large `json!` literal.
#![recursion_limit = "256"]

mod runtime;

use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{Context, Result, anyhow, bail};
use lumen_engine::{
    audio::AudioClip,
    node::{
        EnumDef, InputPortDef, MediaParam, NodeCategory, NodeKind, NodeParams, NodeSchemaDef,
        PortKind, PropertyDef, PropertyKind,
    },
};
use serde_json::{Map, Value, json};

/// The version of the generated definitions' own format.
const SCHEMA_VERSION: u32 = 1;

/// Writes `composition.schema.json` and `runtime.schema.json` into `out_dir`.
pub fn generate(out_dir: &Path) -> Result<()> {
    let schema_json = render_composition_schema_json()?;
    let runtime_json = render_runtime_schema_json()?;
    fs::create_dir_all(out_dir)
        .with_context(|| format!("failed to create output directory `{}`", out_dir.display()))?;
    write_if_changed(&out_dir.join("composition.schema.json"), &schema_json)?;
    write_if_changed(&out_dir.join("runtime.schema.json"), &runtime_json)
}

pub use runtime::render_runtime_schema_json;

/// The composition schema, checked to be a valid JSON Schema.
pub fn render_composition_schema_json() -> Result<String> {
    let schema_json = render_schema()?;
    let schema: Value = serde_json::from_str(&schema_json)?;
    jsonschema::meta::validate(&schema)
        .map_err(|error| anyhow!("generated composition schema is invalid: {error}"))?;
    Ok(schema_json)
}

fn render_schema() -> Result<String> {
    let node_schemas = NodeKind::schemas()
        .into_iter()
        .map(|schema| (schema.kind, schema))
        .collect::<BTreeMap<_, _>>();
    let node_variants = node_schemas
        .values()
        .map(node_schema_json)
        .collect::<Result<Vec<_>>>()?;
    let lumen_version = lumen_engine::SCHEMA_VERSION;
    let lumen_version_pattern = format!("^{}(\\.[0-9]+)?$", lumen_version.replace('.', "\\."));
    let mut schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "x-lumen-schemaVersion": SCHEMA_VERSION,
        "x-lumen-lumenSchemaVersion": lumen_version,
        "x-lumen-expressions": expressions_json(),
        "x-lumen-portKinds": PortKind::ALL.map(PortKind::name),
        "type": "object",
        "required": ["timeline", "render_settings", "nodes", "connections"],
        "additionalProperties": true,
        "properties": {
            "$schema": {
                "description": "URL of the composition schema, for editors.",
                "type": "string"
            },
            "lumenSchemaVersion": {
                "description": format!("The Lumen major.minor the composition was written for, such as `{lumen_version}`; an engine reads only its own line. Absent means the engine's own version."),
                "type": "string",
                "pattern": lumen_version_pattern
            },
            "metadata": {
                "type": "object",
                "properties": {
                    "name": { "description": "Display name of the composition.", "type": "string" }
                },
                "additionalProperties": true
            },
            "timeline": { "$ref": "#/$defs/timeline" },
            "audio": { "$ref": "#/$defs/audio" },
            "render_settings": { "$ref": "#/$defs/renderSettings" },
            "functions": {
                "description": "Expression functions keyed by signature, such as `rise(start)`. Calls expand where they're used.",
                "type": "object",
                "x-lumen-keyType": "`${string}(${string})`",
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
            "timeline": {
                "x-lumen-type": "Timeline",
                "description": "Length and frame rate. `fps` defaults to 30 at the root, and a template's timeline takes the root's `fps`.",
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
                "additionalProperties": false
            },
            "renderSettings": {
                "x-lumen-type": "RenderSettings",
                "description": "The canvas size in pixels.",
                "type": "object",
                "required": ["width", "height"],
                "properties": {
                    "width": { "type": "integer", "minimum": 1, "maximum": 4294967295u32 },
                    "height": { "type": "integer", "minimum": 1, "maximum": 4294967295u32 }
                },
                "additionalProperties": false
            },
            "templateRenderSettings": {
                "x-lumen-type": "TemplateRenderSettings",
                "type": "object",
                "required": ["width", "height"],
                "properties": {
                    "width": { "type": "integer", "minimum": 1, "maximum": 4294967295u32 },
                    "height": { "type": "integer", "minimum": 1, "maximum": 4294967295u32 }
                },
                "additionalProperties": false
            },
            "compositionTemplate": {
                "x-lumen-type": "CompositionTemplate",
                "description": "A composition a `clip` plays. Without `render_settings` it takes the root's size, and without `timeline` the root's timeline; a `timeline` without `fps` takes the root's fps.",
                "type": "object",
                "required": ["nodes", "connections"],
                "properties": {
                    "render_settings": { "$ref": "#/$defs/templateRenderSettings" },
                    "timeline": { "$ref": "#/$defs/timeline" },
                    "audio": { "$ref": "#/$defs/audio" },
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
                "x-lumen-type": "CompositionPropDeclaration",
                "description": "A template prop's type and the value clips that don't set it get. Without a default it is empty, 0, transparent, [0, 0] or [0, 0, 0].",
                "oneOf": [
                    prop_declaration_json("string", json!({ "type": "string" })),
                    prop_declaration_json("number", json!({ "type": "number" })),
                    prop_declaration_json("color", json!({ "$ref": "#/$defs/color" })),
                    prop_declaration_json("vec2", json!({ "$ref": "#/$defs/vec2" })),
                    prop_declaration_json("vec3", json!({ "$ref": "#/$defs/vec3" }))
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
            "vec3": {
                "type": "array",
                "prefixItems": [{ "type": "number" }, { "type": "number" }, { "type": "number" }],
                "items": false,
                "minItems": 3,
                "maxItems": 3
            },
            "connection": {
                "x-lumen-type": "Connection",
                "type": "object",
                "required": ["from_node", "to_node", "to_port"],
                "properties": {
                    "from_node": { "type": "integer", "minimum": 0 },
                    "from_port": {
                        "description": "The output of `from_node` to read. Optional: it defaults to `output`, the only output any node has.",
                        "type": "string"
                    },
                    "to_node": { "type": "integer", "minimum": 0 },
                    "to_port": { "type": "string" }
                },
                "additionalProperties": false
            },
            "audio": {
                "x-lumen-type": "AudioTimeline",
                "description": "Audio tracks and clips. A template's audio plays wherever a `clip` plays the template, following the clip's start, duration, offset, speed and end.",
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
                "additionalProperties": false
            },
            "audioTrack": {
                "x-lumen-type": "AudioTrack",
                "type": "object",
                "required": ["id"],
                "properties": {
                    "id": { "type": "string" },
                    "name": { "type": "string" },
                    "muted": { "type": "boolean" },
                    "solo": { "type": "boolean" },
                    "gain": audio_gain_json()
                },
                "additionalProperties": false
            },
            "audioClip": {
                "x-lumen-type": "AudioClip",
                "type": "object",
                "required": ["id", "source_id", "track_id", "start_ms", "duration_ms"],
                "properties": {
                    "id": { "type": "string" },
                    "source_id": { "type": "string" },
                    "track_id": { "type": "string" },
                    "name": { "type": "string" },
                    "start_ms": { "type": "integer", "minimum": 0 },
                    "duration_ms": { "type": "integer", "minimum": 1 },
                    "source_start_ms": { "type": "integer", "minimum": 0 },
                    "gain": audio_gain_json(),
                    "fade_in_ms": {
                        "description": "Milliseconds the clip rises from silence over at its start, on an equal-power curve.",
                        "type": "integer",
                        "minimum": 0
                    },
                    "fade_out_ms": {
                        "description": "Milliseconds the clip falls to silence over at its end, on an equal-power curve.",
                        "type": "integer",
                        "minimum": 0
                    }
                },
                "additionalProperties": false
            }
        }
    });
    schema["$defs"]
        .as_object_mut()
        .expect("`$defs` is an object")
        .extend(kind_defs_json()?);
    for media in AudioClip::MEDIA_PARAMS {
        schema["$defs"]["audioClip"]["properties"][media.param]["x-lumen-media"] =
            media_json(media);
    }
    Ok(format!("{}\n", serde_json::to_string_pretty(&schema)?))
}

/// An audio track's or clip's gain: a number, or an expression of the composition's time.
fn audio_gain_json() -> Value {
    json!({
        "description": "Gain, 1 by default. An expression is evaluated in the time of the composition the audio is in, and reads `frame`, `time`, `fps`, `width`, `height` and props.",
        "anyOf": [
            { "type": "number", "minimum": 0, "maximum": f32::MAX as f64 },
            { "$ref": "#/$defs/expression" }
        ]
    })
}

/// `$defs` of the param kinds whose fields are a delegate's `property_defs()`, and the outer
/// shapes around them that the property metadata can't express. Each carries the TypeScript
/// type name `x-lumen-type`.
fn kind_defs_json() -> Result<Map<String, Value>> {
    use lumen_engine::node::{
        source::text_content::{
            TextHighlightDelegate, TextLineDelegate, TextSpanDelegate, TextStrokeDelegate,
        },
        vector::{
            dash::{MAX_DASH_LENGTH, MAX_DASH_LENGTHS},
            paint::{GradientPaintDelegate, GradientStopDelegate, MAX_GRADIENT_STOPS},
            repeat::{MAX_REPEAT_COUNT, RepeatDelegate},
        },
    };

    let mut defs = Map::new();
    insert_object_def::<TextSpanDelegate>(&mut defs, "TextSpan", &[])?;
    insert_object_def::<TextLineDelegate>(&mut defs, "TextLine", &[])?;
    insert_object_def::<TextHighlightDelegate>(&mut defs, "TextHighlight", &[])?;
    insert_object_def::<TextStrokeDelegate>(&mut defs, "TextStroke", &[])?;
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
                    "maxItems": MAX_GRADIENT_STOPS,
                    "items": { "$ref": "#/$defs/gradientStop" }
                }),
            ),
        ],
    )?;
    let stop = object_schema_json::<GradientStopDelegate>(&mut defs, &[])?;
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
        "DashPattern",
        json!({
            "description": "Lengths that a stroke alternately draws and skips, starting with a dash. Each takes a literal or an expression.",
            "type": "array",
            "maxItems": MAX_DASH_LENGTHS,
            "items": {
                "anyOf": [
                    { "type": "number", "minimum": 0, "maximum": MAX_DASH_LENGTH },
                    { "$ref": "#/$defs/expression" }
                ]
            }
        }),
    );
    insert_def(
        &mut defs,
        "Region",
        json!({
            "description": "A rectangle of pixels, `[x, y, width, height]`. Each number takes an integer or an expression.",
            "type": "array",
            "prefixItems": [
                { "anyOf": [{ "type": "integer" }, { "$ref": "#/$defs/expression" }] },
                { "anyOf": [{ "type": "integer" }, { "$ref": "#/$defs/expression" }] },
                { "anyOf": [{ "type": "integer" }, { "$ref": "#/$defs/expression" }] },
                { "anyOf": [{ "type": "integer" }, { "$ref": "#/$defs/expression" }] }
            ],
            "items": false,
            "minItems": 4,
            "maxItems": 4
        }),
    );
    insert_object_def::<RepeatDelegate>(
        &mut defs,
        "Repeat",
        &[
            (
                "count",
                json!({ "type": "integer", "minimum": 0, "maximum": MAX_REPEAT_COUNT }),
            ),
            ("columns", json!({ "type": "integer", "minimum": 0 })),
        ],
    )?;
    insert_def(
        &mut defs,
        "FontFeatures",
        json!({
            "description": "OpenType features by four-character tag. 0 turns a feature off, 1 turns it on and larger values select an alternate.",
            "type": "object",
            "propertyNames": { "minLength": 4, "maxLength": 4, "pattern": "^[\\x20-\\x7E]+$" },
            "additionalProperties": {
                "anyOf": [
                    { "type": "integer", "minimum": 0, "maximum": u32::MAX },
                    { "$ref": "#/$defs/expression" }
                ]
            }
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
                { "$ref": "#/$defs/vec3" },
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

fn insert_def(defs: &mut Map<String, Value>, type_name: &str, mut schema: Value) {
    schema["x-lumen-type"] = json!(type_name);
    defs.insert(def_key(type_name), schema);
}

fn insert_object_def<P: NodeParams>(
    defs: &mut Map<String, Value>,
    type_name: &str,
    value_schemas: &[(&str, Value)],
) -> Result<()> {
    let schema = object_schema_json::<P>(defs, value_schemas)?;
    insert_def(defs, type_name, schema);
    Ok(())
}

/// An object schema with a field per param of `P`, described like node params. Fields marked
/// `required` must be present and have no default. Enum fields reference a `$def` of their
/// enum, and `value_schemas` replaces the value schemas of fields the metadata can't describe.
fn object_schema_json<P: NodeParams>(
    defs: &mut Map<String, Value>,
    value_schemas: &[(&str, Value)],
) -> Result<Value> {
    let defaults = P::default().json_properties();
    let mut fields = Map::new();
    let mut required = Vec::new();
    for property in &P::property_defs() {
        let default = if property.required {
            required.push(property.id);
            None
        } else {
            default_json(property, &defaults)
        };
        let replaced = value_schemas.iter().find(|(id, _)| *id == property.id);
        let value_schema = match (replaced, property.enum_def) {
            (Some((_, schema)), _) => schema.clone(),
            (None, Some(enum_def)) => {
                insert_def(
                    defs,
                    enum_def.name,
                    enum_def_schema_json(property, enum_def),
                );
                json!({ "$ref": format!("#/$defs/{}", def_key(enum_def.name)) })
            }
            (None, None) => value_schema_json(property),
        };
        let mut field = field_schema_json(property, default, value_schema);
        // The param kind describes the field's Rust type, not the replaced shape.
        if let (Some(_), Value::Object(object)) = (replaced, &mut field) {
            object.remove("x-lumen-kind");
        }
        fields.insert(property.id.to_string(), field);
    }
    if let Some(id) = value_schemas
        .iter()
        .map(|(id, _)| id)
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
    use lumen_engine::expr::{BUILTINS, FORMS, GLOBALS, NamedEasing, reserved_names};
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
    let easings = NamedEasing::ALL.map(NamedEasing::name);
    let forms = FORMS
        .iter()
        .map(|form| json!({ "name": form.name, "signature": form.signature, "doc": form.doc }))
        .collect::<Vec<_>>();
    let reserved = reserved_names().collect::<Vec<_>>();
    json!({
        "builtins": builtins,
        "easings": easings,
        "forms": forms,
        "globals": globals,
        "reserved": reserved,
    })
}

fn node_schema_json(schema: &NodeSchemaDef) -> Result<Value> {
    let mut properties = Map::new();
    for property in &schema.properties {
        let default = default_json(property, &schema.default_properties);
        properties.insert(
            property.id.to_string(),
            property_schema_json(property, default),
        );
    }
    Ok(json!({
        "title": schema.name,
        "description": schema.description,
        "x-lumen-category": category_name(schema.category),
        "x-lumen-inputs": schema.inputs.iter().map(input_port_json).collect::<Vec<_>>(),
        "x-lumen-outputs": [{ "name": "output", "kind": schema.output.name() }],
        "type": "object",
        "required": ["id", "type"],
        "properties": {
            "id": { "type": "integer", "minimum": 0 },
            "type": { "const": schema.kind },
            "params": {
                "type": "object",
                "properties": properties,
                "additionalProperties": false
            }
        },
        "additionalProperties": false
    }))
}

fn input_port_json(input: &InputPortDef) -> Value {
    json!({
        "name": input.name,
        "kind": input.kind.name(),
        "optional": input.optional,
        "variadic": input.variadic,
    })
}

fn property_schema_json(property: &PropertyDef, default: Option<Value>) -> Value {
    field_schema_json(property, default, value_schema_json(property))
}

fn value_schema_json(property: &PropertyDef) -> Value {
    match property.expected {
        PropertyKind::Float => number_schema_json(property, "number"),
        PropertyKind::Int => number_schema_json(property, "integer"),
        PropertyKind::Bool => json!({ "type": "boolean" }),
        // A single leading `=` parses as an expression; `==` escapes a literal `=`.
        PropertyKind::String if !property.accepts_expressions => {
            json!({ "type": "string", "not": { "pattern": "^=([^=]|$)" } })
        }
        PropertyKind::String => json!({ "type": "string" }),
        PropertyKind::Color => json!({ "$ref": "#/$defs/color" }),
        PropertyKind::Paint => json!({ "$ref": "#/$defs/paint" }),
        PropertyKind::Vec2 => json!({ "$ref": "#/$defs/vec2" }),
        PropertyKind::Vec3 => json!({ "$ref": "#/$defs/vec3" }),
        PropertyKind::ShaderUniforms => json!({ "$ref": "#/$defs/shaderUniforms" }),
        PropertyKind::ClipProps => json!({ "$ref": "#/$defs/clipProps" }),
        PropertyKind::TextContent => json!({ "$ref": "#/$defs/textContent" }),
        PropertyKind::TextLine => json!({ "$ref": "#/$defs/textLine" }),
        PropertyKind::TextHighlight => json!({ "$ref": "#/$defs/textHighlight" }),
        PropertyKind::FontFeatures => json!({ "$ref": "#/$defs/fontFeatures" }),
        PropertyKind::TextStroke => json!({ "$ref": "#/$defs/textStroke" }),
        PropertyKind::DashPattern => json!({ "$ref": "#/$defs/dashPattern" }),
        PropertyKind::Region => json!({ "$ref": "#/$defs/region" }),
        PropertyKind::Repeat => json!({ "$ref": "#/$defs/repeat" }),
        PropertyKind::Enum => enum_schema_json(property),
    }
}

fn number_schema_json(property: &PropertyDef, kind: &str) -> Value {
    match property.constraints.min {
        Some(minimum) if property.constraints.enforced_min => {
            json!({ "type": kind, "minimum": minimum })
        }
        _ => json!({ "type": kind }),
    }
}

/// The schema of an enum param, which names its Rust enum as `x-lumen-type` so that params
/// sharing an enum share a generated type.
fn enum_schema_json(property: &PropertyDef) -> Value {
    let Some(enum_def) = property.enum_def else {
        return json!({ "type": "string", "enum": [] });
    };
    let values = enum_def.options.iter().map(|option| option.name);
    json!({
        "type": "string",
        "enum": values.collect::<Vec<_>>(),
        "x-lumen-type": enum_def.name,
    })
}

/// The schema of an enum `$def`, which also lists the serde aliases the engine accepts.
fn enum_def_schema_json(property: &PropertyDef, enum_def: &EnumDef) -> Value {
    let mut schema = enum_schema_json(property);
    if let Some(values) = schema["enum"].as_array_mut() {
        let aliases = enum_def.options.iter().flat_map(|option| option.aliases);
        values.extend(aliases.map(|alias| json!(alias)));
    }
    schema
}

/// The schema of an `Override` field: absent inherits, `null` switches the inherited value off.
fn nullable_schema_json(schema: Value) -> Value {
    json!({ "anyOf": [schema, { "type": "null" }] })
}

/// `value_schema` with the param's expression, `null` and metadata keywords.
fn field_schema_json(property: &PropertyDef, default: Option<Value>, value_schema: Value) -> Value {
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
        object.insert(
            "x-lumen-kind".to_string(),
            json!(property_kind_name(property.expected)),
        );
        if let Some(default) = default {
            object.insert("default".to_string(), default);
        }
        if let Some(enum_def) = property.enum_def
            && !enum_def.options.is_empty()
        {
            let options = enum_def.options.iter().map(|option| {
                json!({ "name": option.name, "label": option.label, "value": option.value })
            });
            object.insert(
                "x-lumen-enumOptions".to_string(),
                options.collect::<Vec<_>>().into(),
            );
        }
        if let Some(constraints) = constraints_json(property) {
            object.insert("x-lumen-constraints".to_string(), constraints);
        }
        if let Some(media) = &property.media {
            object.insert("x-lumen-media".to_string(), media_json(media));
        }
    }
    schema
}

/// The `x-lumen-media` of a param that names media: the kinds it accepts, and the param whose
/// value picks among them.
fn media_json(media: &MediaParam) -> Value {
    let mut json =
        json!({ "kinds": media.kinds.iter().map(|kind| kind.name()).collect::<Vec<_>>() });
    if let Some(selector) = media.selector {
        json["selectedBy"] = json!(selector);
    }
    json
}

/// The editor hints a property sets, or `None` when it sets none.
fn constraints_json(property: &PropertyDef) -> Option<Value> {
    let constraints = &property.constraints;
    let mut hints = Map::new();
    if let Some(min) = constraints.min {
        hints.insert("min".to_string(), json!(min));
    }
    if let Some(max) = constraints.max {
        hints.insert("max".to_string(), json!(max));
    }
    if let Some(step) = constraints.step {
        hints.insert("step".to_string(), json!(step));
    }
    if let Some(format) = constraints.format {
        hints.insert("format".to_string(), json!(format));
    }
    if constraints.multiline {
        hints.insert("multiline".to_string(), json!(true));
    }
    if let Some(rows) = constraints.recommended_rows {
        hints.insert("recommendedRows".to_string(), json!(rows));
    }
    if let Some(role) = constraints.role {
        hints.insert("role".to_string(), json!(role));
    }
    (!hints.is_empty()).then_some(Value::Object(hints))
}

fn category_name(category: NodeCategory) -> &'static str {
    match category {
        NodeCategory::Compositing => "compositing",
        NodeCategory::Processing => "processing",
        NodeCategory::Source => "source",
        NodeCategory::Output => "output",
        NodeCategory::Vector => "vector",
        NodeCategory::Scene => "scene",
    }
}

fn property_kind_name(kind: PropertyKind) -> &'static str {
    match kind {
        PropertyKind::Float => "float",
        PropertyKind::Int => "int",
        PropertyKind::Bool => "bool",
        PropertyKind::String => "string",
        PropertyKind::Color => "color",
        PropertyKind::Paint => "paint",
        PropertyKind::Vec2 => "vec2",
        PropertyKind::Vec3 => "vec3",
        PropertyKind::Enum => "enum",
        PropertyKind::ShaderUniforms => "shader_uniforms",
        PropertyKind::ClipProps => "clip_props",
        PropertyKind::TextContent => "text_content",
        PropertyKind::TextLine => "text_line",
        PropertyKind::TextHighlight => "text_highlight",
        PropertyKind::FontFeatures => "font_features",
        PropertyKind::TextStroke => "text_stroke",
        PropertyKind::DashPattern => "dash_pattern",
        PropertyKind::Region => "region",
        PropertyKind::Repeat => "repeat",
    }
}

/// `property`'s entry in `defaults`, which is absent for optional params and for optional fields
/// of a param kind, since they are unset unless given.
fn default_json(property: &PropertyDef, defaults: &[(&'static str, Value)]) -> Option<Value> {
    defaults
        .iter()
        .find(|(name, _)| *name == property.id)
        .map(|(_, value)| value.clone())
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
